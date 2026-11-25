// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The files a draft holds. Each shows as a pill the moment it arrives and
// stages on the agent's device in the background, so the message only goes
// once every placed file is staged. On a remote device the file uploads
// instead, and the pill shows how far it has got. A file pulled out of
// another agent's timeline copies from that agent's device first, so it
// shows how far it has got too. A file whose pill you delete stays here
// while undo can bring it back. Each composer has its own.

import { createStore, type StoreApi } from "zustand/vanilla";

import type { ActionResult, DataSource, TransferProgress } from "@maxhq/shared/data/source";
import type {
	AgentId,
	AttachmentId,
	AttachmentRef,
	DeviceId,
	TransferId,
	UploadTarget,
} from "@maxhq/shared/bindings";

import { refusalFor } from "./limits";
import { type ComposerNative, type FileKind, guessPath, type PathInfo } from "./native";
import { nameOf } from "@maxhq/shared/folders/path";
import { failureOf } from "@maxhq/shared/composer/send";
import { formatNumber, t as translate } from "@maxhq/shared/i18n";

/** Where a chip is on its way to the agent. Uploading means it copies to a remote device. */
export type ChipState = "staging" | "uploading" | "staged" | "failed";

/** How far a chip's copy to a remote device has got. */
export type ChipUpload = {
	/** Known once the first progress comes in. */
	readonly transfer: TransferId | null;
	readonly sent: number;
	readonly total: number;
	readonly state: TransferProgress["state"];
};

/** One file or folder waiting to go out. */
export type Chip = {
	/** Stable for the chip's life. */
	readonly key: string;
	readonly name: string;
	readonly kind: FileKind;
	/** The size, once known. A folder counts every file inside it. */
	readonly bytes: number | null;
	readonly mime: string | null;
	/** The dropped, pasted or picked path, which the native thumbnail can read. */
	readonly source: string | null;
	/** An object URL for pasted image bytes, revoked when the chip goes. */
	readonly preview: string | null;
	readonly state: ChipState;
	/** Set once staged. */
	readonly ref: AttachmentRef | null;
	/** Why staging failed. */
	readonly error: string | null;
	/** Set for a file that copies to a remote device, and kept once it has. */
	readonly upload: ChipUpload | null;
	/** Whether Retry can try a failed chip again. */
	readonly retryable: boolean;
	/** A visual's page, for a chip that carries one, so its pill draws it. */
	readonly page?: string;
};

/** What staging needs to reach the agent's device. */
export type StageContext = {
	readonly source: DataSource;
	readonly device: DeviceId;
	readonly native: ComposerNative;
	/** Where files upload to on a remote device, or null when the agent runs on this Mac. */
	readonly target?: UploadTarget | null;
};

/** A file on an agent's device, which that agent can read, such as one in its timeline. */
export type AgentFile = {
	readonly agent: AgentId;
	/** Absolute, or from the agent's folder. */
	readonly path: string;
	readonly name: string;
	readonly kind: FileKind;
	readonly bytes: number | null;
	readonly mime: string | null;
};

/** A moment and how many bytes had uploaded by then. */
export type RateSample = { readonly at: number; readonly sent: number };

/** How far back the upload speed looks. */
export const RATE_WINDOW_MS = 3000;

/** The chips and what changes them. */
export type AttachmentsState = {
	readonly chips: readonly Chip[];
	/** The bytes uploaded over the last few seconds, counting every chip, for the speed. */
	readonly samples: readonly RateSample[];
	/**
	 * Adds a chip for each path at once and stages or uploads them to the
	 * device. Returns their keys, in order. With `foundIn`, the paths are
	 * ones the `@` menu found in that agent's folder, relative to it, so
	 * they stage there on its device and nothing uploads.
	 */
	readonly addPaths: (
		context: StageContext,
		paths: readonly string[],
		options?: { readonly foundIn?: AgentId }
	) => string[];
	/**
	 * Adds a file from an agent's device. It copies from there, then stages
	 * on this agent's device or uploads to it. Returns its key. Each call
	 * adds a chip of its own, even for a file already here.
	 */
	readonly addFromAgent: (context: StageContext, file: AgentFile) => string;
	/** Stages or uploads pasted or dropped bytes as a file. Returns its key. */
	readonly addBytes: (
		context: StageContext,
		name: string,
		mime: string,
		bytes: Uint8Array
	) => string;
	/** Stages or uploads a visual's page as an HTML file, keeping the page. Returns its key. */
	readonly addPage: (context: StageContext, name: string, html: string) => string;
	/**
	 * Adds a sent message's attachments again, such as a queued message's
	 * when you edit it. maxd already moved them into that agent's folder,
	 * and a message to the same agent may name them again. Returns their
	 * keys, in order.
	 */
	readonly restore: (refs: readonly AttachmentRef[]) => string[];
	/** Drops a chip and stops its upload if it has one on its way. */
	readonly remove: (key: string) => void;
	/** Tries a failed upload again. */
	readonly retry: (key: string) => void;
	/** Stops every upload still on its way and drops its chips. */
	readonly cancelUploads: () => void;
	/** Drops every chip, once the message is sent. */
	readonly clear: () => void;
	/** Resolves once every chip, or each of `keys`, has staged or failed. */
	readonly settled: (keys?: readonly string[]) => Promise<void>;
};

/** A composer's chips. */
export type AttachmentsStore = StoreApi<AttachmentsState>;

const IMAGE_MIME = /^image\//;

const NO_UPLOAD = "Couldn't upload it";

/** The staged attachments' IDs, in order. */
export const stagedIds = (chips: readonly Chip[]): AttachmentId[] =>
	chips.flatMap((chip) => (chip.state === "staged" && chip.ref ? [chip.ref.id] : []));

/** The chips with these keys, in the keys' order, skipping any that are gone. */
export const chipsOf = (chips: readonly Chip[], keys: readonly string[]): Chip[] => {
	const byKey = new Map(chips.map((chip) => [chip.key, chip]));
	return keys.flatMap((key) => {
		const chip = byKey.get(key);
		return chip ? [chip] : [];
	});
};

/** Whether any chip is still on its way, staging or uploading. */
export const anyStaging = (chips: readonly Chip[]): boolean =>
	chips.some((chip) => chip.state === "staging" || chip.state === "uploading");

/** Whether any chip is still uploading to a remote device. */
export const anyUploading = (chips: readonly Chip[]): boolean =>
	chips.some((chip) => chip.state === "uploading");

/** A staged chip that keeps the page the one before it carried. */
const keepPage = (before: Chip, after: Chip): Chip =>
	before.page === undefined ? after : { ...after, page: before.page };

const fromRef = (
	key: string,
	ref: AttachmentRef,
	source: string | null,
	preview: string | null,
	upload: ChipUpload | null = null
) =>
	({
		key,
		name: ref.name,
		kind: ref.kind,
		bytes: ref.bytes,
		mime: ref.mime,
		source,
		preview,
		state: "staged",
		ref,
		error: null,
		upload,
		retryable: false,
	}) satisfies Chip;

/** A chip about to start uploading. */
const starting = (bytes: number | null): ChipUpload => ({
	transfer: null,
	sent: 0,
	total: bytes ?? 0,
	state: "uploading",
});

/** A chip that failed, with Retry or without. */
const failed = (chip: Chip, error: string, retryable: boolean): Chip => ({
	...chip,
	state: "failed",
	error,
	retryable,
	upload: chip.upload && { ...chip.upload, state: "failed" },
});

/** What an upload needs to run again. */
type Job = {
	readonly source: DataSource;
	/** Starts the copy, telling `onProgress` how far it has got. */
	readonly start: (
		onProgress: (progress: TransferProgress) => void
	) => Promise<ActionResult | undefined>;
};

/** A new composer's empty set of chips. */
export const createAttachments = (): AttachmentsStore => {
	let next = 0;
	let runs = 0;
	/** Every byte uploaded so far, which only grows, so the speed holds as files finish. */
	let moved = 0;
	const newKey = () => `chip-${++next}`;
	/** How to run each chip's upload again, for Retry. */
	const jobs = new Map<string, Job>();
	/** Each uploading chip's current run, so a late answer from an older one is ignored. */
	const runOf = new Map<string, number>();
	const stopped = new Set<TransferId>();

	const stop = (source: DataSource, transfer: TransferId) => {
		if (stopped.has(transfer)) return;
		stopped.add(transfer);
		source.cancelTransfer(transfer).catch(console.error);
	};

	return createStore<AttachmentsState>((set, get, api) => {
		const find = (key: string) => get().chips.find((chip) => chip.key === key);

		const patch = (key: string, change: (chip: Chip) => Chip) =>
			set({ chips: get().chips.map((chip) => (chip.key === key ? change(chip) : chip)) });

		const settle = (key: string, result: ActionResult | undefined) => {
			if (result?.status === "ok" && result.data.status === "staged") {
				const ref = result.data.attachment;
				patch(key, (chip) =>
					keepPage(chip, fromRef(chip.key, ref, chip.source, chip.preview))
				);
				return;
			}
			const error = (result && failureOf(result)) ?? translate("Couldn't get it ready");
			patch(key, (chip) => failed(chip, error, false));
		};

		const fail = (keys: readonly string[], error: unknown) => {
			const message =
				error instanceof Error ? error.message : translate("Couldn't get it ready");
			for (const key of keys) patch(key, (chip) => failed(chip, message, false));
		};

		/** Whether an answer belongs to the chip's current run and the chip still uploads. */
		const current = (key: string, run: number) =>
			runOf.get(key) === run && find(key)?.state === "uploading";

		const uploaded = (key: string, run: number, result: ActionResult | undefined) => {
			if (!current(key, run)) return;
			runOf.delete(key);
			if (result?.status === "ok" && result.data.status === "staged") {
				jobs.delete(key);
				const ref = result.data.attachment;
				patch(key, (chip) => {
					const total = Math.max(chip.upload?.total ?? 0, ref.bytes ?? 0);
					const upload: ChipUpload = {
						transfer: chip.upload?.transfer ?? null,
						sent: total,
						total,
						state: "done",
					};
					return keepPage(
						chip,
						fromRef(chip.key, ref, chip.source, chip.preview, upload)
					);
				});
				return;
			}
			const refused = result?.status === "ok" && result.data.status === "refused";
			const error = (result && failureOf(result)) ?? NO_UPLOAD;
			patch(key, (chip) => failed(chip, error, !refused));
		};

		const run = (key: string, job: Job) => {
			const chip = find(key);
			if (chip?.state !== "uploading") return;
			jobs.set(key, job);
			const refusal = refusalFor(get().chips, key);
			if (refusal) {
				patch(key, (each) => failed(each, refusal, false));
				return;
			}
			const id = ++runs;
			runOf.set(key, id);
			const { source } = job;

			const onProgress = (progress: TransferProgress) => {
				if (!current(key, id)) {
					stop(source, progress.transfer);
					return;
				}
				const upload: ChipUpload = {
					transfer: progress.transfer,
					sent: progress.sent,
					total: progress.total,
					state: progress.state,
				};
				const chips = get().chips.map((each) =>
					each.key === key ? { ...each, upload } : each
				);
				const over = refusalFor(chips, key);
				if (over) {
					runOf.delete(key);
					stop(source, progress.transfer);
					patch(key, (each) => failed({ ...each, upload }, over, false));
					return;
				}
				const before = find(key)?.upload?.sent ?? 0;
				moved += Math.max(0, progress.sent - before);
				const at = Date.now();
				const samples = [
					...get().samples.filter((sample) => at - sample.at <= RATE_WINDOW_MS),
					{ at, sent: moved },
				];
				set({ chips, samples });
			};

			job.start(onProgress).then(
				(result) => uploaded(key, id, result),
				(error: unknown) => {
					if (!current(key, id)) return;
					runOf.delete(key);
					const message = error instanceof Error ? error.message : NO_UPLOAD;
					patch(key, (each) => failed(each, message, true));
				}
			);
		};

		return {
			chips: [],
			samples: [],

			addPaths: (context, paths, options = {}) => {
				if (paths.length === 0) return [];
				const { source, device, native } = context;
				const foundIn = options.foundIn ?? null;
				const target = foundIn === null ? context.target : null;
				const added = paths.map((path): Chip => {
					const guess = describe(guessPath(path));
					return {
						...guess,
						key: newKey(),
						// The native thumbnail can only read a path on this Mac.
						source: foundIn === null ? path : null,
						preview: null,
						state: target ? "uploading" : "staging",
						ref: null,
						error: null,
						upload: target ? starting(guess.bytes) : null,
						retryable: false,
					};
				});
				const keys = added.map((chip) => chip.key);
				set({ chips: [...get().chips, ...added] });

				if (foundIn !== null) {
					const found = paths.map((path) => path.replace(/(.)\/+$/, "$1"));
					source.stageFound(foundIn, found).then(
						(results) => keys.forEach((key, index) => settle(key, results[index])),
						(error: unknown) => fail(keys, error)
					);
					return keys;
				}

				const described = native.describePaths(paths).then((infos) => {
					infos.forEach((info, index) => {
						const key = keys[index];
						if (key === undefined) return;
						patch(key, (chip) =>
							chip.state === "staging" || chip.state === "uploading"
								? { ...chip, ...describe(info) }
								: chip
						);
					});
				}, console.error);

				if (target) {
					// Sizes come from the description, so the limits can turn a file
					// away before any of it copies.
					void described.then(() =>
						keys.forEach((key, index) => {
							const path = paths[index];
							if (path === undefined) return;
							run(key, {
								source,
								start: (onProgress) =>
									source
										.upload(device, target, [path], onProgress)
										.then(([first]) => first),
							});
						})
					);
					return keys;
				}

				source.stageAttachments(device, [...paths]).then(
					(results) => keys.forEach((key, index) => settle(key, results[index])),
					(error: unknown) => fail(keys, error)
				);
				return keys;
			},

			addBytes: ({ source, device, target }, name, mime, bytes) => {
				const key = newKey();
				const preview = IMAGE_MIME.test(mime)
					? URL.createObjectURL(
							new Blob([bytes as Uint8Array<ArrayBuffer>], { type: mime })
						)
					: null;
				const chip: Chip = {
					key,
					name,
					kind: IMAGE_MIME.test(mime)
						? "image"
						: mime === "application/pdf"
							? "pdf"
							: "file",
					bytes: bytes.byteLength,
					mime,
					source: null,
					preview,
					state: target ? "uploading" : "staging",
					ref: null,
					error: null,
					upload: target ? starting(bytes.byteLength) : null,
					retryable: false,
				};
				set({ chips: [...get().chips, chip] });
				if (target) {
					run(key, {
						source,
						start: (onProgress) =>
							source.uploadPasted(device, target, name, mime, bytes, onProgress),
					});
					return key;
				}
				source.stagePasted(device, name, mime, bytes).then(
					(result) => settle(key, result),
					(error: unknown) => fail([key], error)
				);
				return key;
			},

			addPage: (context, name, html) => {
				const key = get().addBytes(
					context,
					name,
					"text/html",
					new TextEncoder().encode(html)
				);
				patch(key, (chip) => ({ ...chip, page: html }));
				return key;
			},

			addFromAgent: ({ source, device, target = null }, file) => {
				const key = newKey();
				const chip: Chip = {
					key,
					// The path is what gets copied, so the chip names that and
					// not the name the timeline item gave it.
					name: nameOf(file.path),
					kind: file.kind,
					bytes: file.bytes,
					mime: file.mime,
					source: null,
					preview: null,
					state: "uploading",
					ref: null,
					error: null,
					upload: starting(file.bytes),
					retryable: false,
				};
				set({ chips: [...get().chips, chip] });
				run(key, {
					source,
					start: (onProgress) =>
						source.attachFromAgent(device, target, file.agent, file.path, onProgress),
				});
				return key;
			},

			restore: (refs) => {
				const added = refs.map((ref) => fromRef(newKey(), ref, null, null));
				set({ chips: [...get().chips, ...added] });
				return added.map((chip) => chip.key);
			},

			remove: (key) => {
				const gone = find(key);
				const job = jobs.get(key);
				jobs.delete(key);
				runOf.delete(key);
				if (job && gone?.state === "uploading" && gone.upload?.transfer)
					stop(job.source, gone.upload.transfer);
				if (gone?.preview) URL.revokeObjectURL(gone.preview);
				set({ chips: get().chips.filter((chip) => chip.key !== key) });
			},

			retry: (key) => {
				const chip = find(key);
				const job = jobs.get(key);
				if (chip?.state !== "failed" || !chip.retryable || !job) return;
				patch(key, (each) => ({
					...each,
					state: "uploading",
					error: null,
					retryable: false,
					upload: starting(each.bytes),
				}));
				run(key, job);
			},

			cancelUploads: () => {
				const { remove } = get();
				for (const chip of get().chips) if (chip.state === "uploading") remove(chip.key);
			},

			clear: () => {
				get().cancelUploads();
				for (const chip of get().chips) if (chip.preview) URL.revokeObjectURL(chip.preview);
				jobs.clear();
				runOf.clear();
				set({ chips: [] });
			},

			settled: (keys) =>
				new Promise<void>((resolve) => {
					const waiting = (chips: readonly Chip[]) =>
						anyStaging(keys ? chipsOf(chips, keys) : chips);
					if (!waiting(get().chips)) {
						resolve();
						return;
					}
					const unsubscribe = api.subscribe(({ chips }) => {
						if (waiting(chips)) return;
						unsubscribe();
						resolve();
					});
				}),
		};
	});
};

/** The parts of a chip a path's description sets. */
const describe = ({ name, kind, size, mime }: PathInfo) => ({
	name,
	kind,
	bytes: size,
	mime,
});

/** A size as the chips show it, such as "4 KB" or "1.3 MB". */
export const formatBytes = (bytes: number): string => {
	if (bytes < 1024) return `${formatNumber(bytes)} B`;
	const kb = bytes / 1024;
	if (kb < 1000)
		return translate("{{value1}} KB", { value1: formatNumber(Math.max(1, Math.round(kb))) });
	const mb = kb / 1024;
	if (mb < 1000)
		return translate("{{value1}} MB", {
			value1: formatNumber(mb, { minimumFractionDigits: 1, maximumFractionDigits: 1 }),
		});
	return translate("{{value1}} GB", {
		value1: formatNumber(mb / 1024, { minimumFractionDigits: 1, maximumFractionDigits: 1 }),
	});
};

const LANGUAGES: Record<string, string> = {
	rs: "Rust",
	ts: "TypeScript",
	tsx: "TypeScript",
	js: "JavaScript",
	jsx: "JavaScript",
	mjs: "JavaScript",
	py: "Python",
	go: "Go",
	swift: "Swift",
	rb: "Ruby",
	java: "Java",
	kt: "Kotlin",
	c: "C",
	h: "C",
	cpp: "C++",
	cs: "C#",
	sh: "Shell",
	json: "JSON",
	toml: "TOML",
	yaml: "YAML",
	yml: "YAML",
	md: "Markdown",
	html: "HTML",
	css: "CSS",
	sql: "SQL",
	txt: "Text",
	log: "Log",
	csv: "CSV",
};

const extensionOf = (name: string): string => {
	const dot = name.lastIndexOf(".");
	return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
};

/** What kind of file a chip holds, such as "Rust" or "PNG". */
export const chipType = (chip: Pick<Chip, "name" | "kind" | "mime">): string => {
	switch (chip.kind) {
		case "folder":
			return translate("Folder");
		case "pdf":
			return "PDF";
		case "missing":
			return translate("Missing");
		case "image": {
			const subtype = chip.mime?.split("/")[1] ?? extensionOf(chip.name);
			return subtype ? subtype.replace(/^jpeg$/, "jpg").toUpperCase() : translate("Image");
		}
		case "file": {
			const extension = extensionOf(chip.name);
			return (
				LANGUAGES[extension] ?? (extension ? extension.toUpperCase() : translate("File"))
			);
		}
	}
};

/** How far a transfer has got in the total's unit, such as "0.8 of 1.2 MB". */
export const formatProgress = (sent: number, total: number): string => {
	if (total < 1024)
		return translate("{{sent}} of {{total}} B", {
			sent: formatNumber(sent),
			total: formatNumber(total),
		});
	if (total < 1000 * 1024) {
		const kb = (bytes: number) => Math.round(bytes / 1024);
		return translate("{{kb}} of {{value2}} KB", {
			kb: formatNumber(kb(sent)),
			value2: formatNumber(Math.max(1, kb(total))),
		});
	}
	const mib = 1024 * 1024;
	const [unit, name] = total < 1000 * mib ? [mib, "MB"] : [1024 * mib, "GB"];
	return translate("{{value1}} of {{value2}} {{name}}", {
		value1: formatNumber(sent / unit, { minimumFractionDigits: 1, maximumFractionDigits: 1 }),
		value2: formatNumber(total / unit, { minimumFractionDigits: 1, maximumFractionDigits: 1 }),
		name: name,
	});
};

/** Where an upload is, for its chip. */
const uploadDetail = ({ sent, total, state }: ChipUpload): string => {
	if (state === "paused") return translate("Paused");
	if (state === "waiting") return translate("Waiting for room");
	if (total <= 0) return translate("Starting upload");
	return formatProgress(sent, total);
};

/** The chip's second line: what it is and its size, or where staging is. */
export const chipDetail = (chip: Chip): string => {
	if (chip.state === "failed") return chip.error ?? translate("Couldn't get it ready");
	if (chip.state === "staging") return translate("Getting it ready");
	if (chip.state === "uploading") return uploadDetail(chip.upload ?? starting(chip.bytes));
	if (chip.upload)
		return chip.bytes === null
			? translate("Copied")
			: translate("Copied, {{bytes}}", { bytes: formatBytes(chip.bytes) });
	const type = chipType(chip);
	return chip.bytes === null ? type : `${type}, ${formatBytes(chip.bytes)}`;
};

/** The footer's count, such as "4 attachments". */
export const chipCount = (chips: readonly Chip[]): string =>
	chips.length === 1
		? translate("1 attachment")
		: translate("{{length}} attachments", { length: chips.length });

/** The footer's count with the size, such as "4 attachments, 1.3 MB". */
export const chipSummary = (chips: readonly Chip[]): string => {
	const count = chipCount(chips);
	const sizes = chips.map((chip) => chip.bytes);
	if (sizes.some((size) => size === null)) return count;
	const total = sizes.reduce<number>((sum, size) => sum + (size ?? 0), 0);
	return `${count}, ${formatBytes(total)}`;
};
