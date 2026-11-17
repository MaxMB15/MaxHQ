// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The files a phone's box holds, each on its way to the agent's device.
// A photo or a file you pick uploads there from the phone's caches folder,
// where the pickers copy it, and shows how far it has got, up to the 2 GB
// a message carries. To a device that doesn't take uploads it goes as its
// bytes instead, up to the 8 MiB maxd takes in one message. A file the `@`
// menu found stages from the agent's own folder. A draft that comes back
// brings its files' staged copies with it while maxd still keeps them, and
// stages a found file again once its copy has gone.

import { createStore, type StoreApi } from "zustand/vanilla";

import type { AgentId, AttachmentKind, AttachmentRef, DeviceId, TransferId } from "../bindings";
import { MAX_INLINE_ATTACHMENT } from "../bindings";
import { failureOf } from "../data/failure";
import type { ActionResult, DataSource, TransferProgress } from "../data/source";
import { t } from "../i18n";
import type { ClipboardFile, ClipboardFound } from "../pills/clipboard";
import type { FileFacts } from "../pills/draft";
import type { ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";
import { type Counted, overLimit } from "./limits";

/** The most a phone sends of one file as its bytes, as maxd takes it in one message. */
export const MAX_PHONE_FILE = MAX_INLINE_ATTACHMENT;

/** What a file too big for the phone to send says. */
export const tooLargeText = (): string =>
	t("Too large to send from the phone yet (up to {{size}} MB)", {
		size: MAX_PHONE_FILE / (1024 * 1024),
	});

/** Whether a file of `size` bytes is too big to send from the phone as its bytes. */
export const tooLarge = (size: number | null | undefined): boolean =>
	size !== null && size !== undefined && size > MAX_PHONE_FILE;

/** Whether `device` takes uploads, so a picked file of any size can go there. */
export const takesFiles = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.transfers === true;

/** How far a picked file's upload has got. */
export type StagedUpload = {
	/** Known once the first progress comes in. */
	readonly transfer: TransferId | null;
	readonly sent: number;
	readonly total: number;
	readonly state: TransferProgress["state"];
};

/** One file in the box. */
export type StagedFile = {
	readonly key: string;
	readonly name: string;
	readonly kind: AttachmentKind | "missing";
	/** Its size, once known. */
	readonly bytes: number | null;
	/** A picture to show while it stages, such as the photo's own file. */
	readonly preview: string | null;
	readonly state: "staging" | "ready" | "failed";
	/** The staged copy, once it's ready. */
	readonly ref: AttachmentRef | null;
	/** Why it isn't ready, once it failed. */
	readonly error: string | null;
	/** Where the agent can see it, for a file the `@` menu found. */
	readonly found: ClipboardFound | null;
	/** How far its upload has got, for a picked file that uploads. */
	readonly upload: StagedUpload | null;
};

/** A photo or file picked on the phone, with a way to read its bytes. */
export type PickedFile = {
	readonly name: string;
	readonly mime: string;
	/** Its size, when the picker knows it. */
	readonly size: number | null;
	readonly preview: string | null;
	/** Its path in the phone's caches folder, which can upload, or null when only its bytes can go. */
	readonly path: string | null;
	readonly read: () => Promise<Uint8Array>;
};

export type StagingState = {
	readonly files: readonly StagedFile[];
	/**
	 * Uploads a picked file when `uploads` says the device takes uploads,
	 * and stages it as bytes otherwise. Returns its key, or why it stays
	 * out, such as a file over the limit.
	 */
	readonly addPicked: (
		file: PickedFile,
		uploads: boolean
	) => { key: string } | { refused: string };
	/** Stages a file or folder the `@` menu found in an agent's folder. Returns its key. */
	readonly addFound: (agent: AgentId, device: DeviceId, path: string) => string;
	/** Adds a ready file, such as one a queued message already carries. */
	readonly addReady: (ref: AttachmentRef) => string;
	/** Brings back a saved draft's files, staging a found one again when its copy has gone. */
	readonly restore: (files: readonly ClipboardFile[]) => void;
	/** Takes a file out, and stops its upload if it's still on its way. */
	readonly remove: (key: string) => void;
	/** Keeps only these files, as after the text lost a pill, and stops the others' uploads. */
	readonly keepOnly: (keys: ReadonlySet<string>) => void;
	readonly clear: () => void;
};

export type Staging = StoreApi<StagingState>;

/** A file's kind from its media type, until maxd says. */
export const kindOfMime = (mime: string): AttachmentKind =>
	mime.startsWith("image/") ? "image" : mime === "application/pdf" ? "pdf" : "file";

const nameOfPath = (path: string): string => {
	const parts = path.replace(/\/+$/, "").split("/");
	return parts.at(-1) || path;
};

let made = 0;

/** A key no other file in this run has. */
const newKey = (): string => `phone-${Date.now().toString(36)}-${(made += 1)}`;

const couldntReady = () => t("Couldn't get it ready");

/** The facts a file's pill and marker show. */
export const factsOfStaged = (file: StagedFile): FileFacts => ({
	name: file.ref?.name ?? file.name,
	kind: file.ref?.kind ?? file.kind,
});

/** A file as a saved draft keeps it. */
export const clipboardFileOf = (file: StagedFile, device: DeviceId): ClipboardFile => ({
	key: file.key,
	name: file.ref?.name ?? file.name,
	kind: file.ref?.kind ?? file.kind,
	source: null,
	ref: file.ref,
	device: file.ref ? device : null,
	found: file.found,
});

/** The ID of each ready file, by key. */
export const readyId = (files: readonly StagedFile[], key: string) =>
	files.find((file) => file.key === key && file.state === "ready")?.ref?.id ?? null;

/** How far a file's upload has got, 0 to 100, or null when it isn't uploading or its size is unknown. */
export const uploadPercent = (file: StagedFile): number | null =>
	file.state === "staging" && file.upload && file.upload.total > 0
		? Math.min(100, Math.floor((file.upload.sent / file.upload.total) * 100))
		: null;

/** What a file shows under its name while it uploads, or null when it isn't uploading. */
export const uploadText = (file: StagedFile): string | null => {
	if (file.state !== "staging" || !file.upload) return null;
	if (file.upload.state === "paused") return t("Paused while the link is down");
	if (file.upload.state === "waiting") return t("Waiting for room on the device");
	const percent = uploadPercent(file);
	return percent === null ? null : t("{{percent}}% sent", { percent });
};

/** The files as the message limits count them. A failed file never goes. */
const countedOf = (files: readonly StagedFile[]): Counted[] =>
	files.map((file) => ({
		key: file.key,
		bytes: Math.max(file.bytes ?? 0, file.upload?.total ?? 0),
		counts: file.state !== "failed",
	}));

/** Files for `agent` on `device`, staged or uploaded through `source`. */
export const createStaging = (
	source: Pick<DataSource, "stagePasted" | "stageFound" | "upload" | "cancelTransfer">,
	device: DeviceId,
	agent: AgentId,
	key: () => string = newKey
): Staging =>
	createStore<StagingState>()((set, get) => {
		const find = (target: string) => get().files.find((file) => file.key === target);

		const patch = (target: string, change: (file: StagedFile) => StagedFile) =>
			set({
				files: get().files.map((file) => (file.key === target ? change(file) : file)),
			});

		const stopped = new Set<TransferId>();
		const stop = (transfer: TransferId) => {
			if (stopped.has(transfer)) return;
			stopped.add(transfer);
			// It may have finished already, which leaves nothing to stop.
			source.cancelTransfer(transfer).catch(() => undefined);
		};

		/** Stops the uploads of these files, which are leaving the box. */
		const stopAll = (leaving: readonly StagedFile[]) => {
			for (const file of leaving)
				if (file.state === "staging" && file.upload?.transfer) stop(file.upload.transfer);
		};

		const settle = (target: string, result: ActionResult | undefined) => {
			if (result?.status === "ok" && result.data.status === "staged") {
				const ref = result.data.attachment;
				patch(target, (file) => ({
					...file,
					name: ref.name,
					kind: ref.kind,
					bytes: ref.bytes,
					state: "ready",
					ref,
					error: null,
				}));
				return;
			}
			const error = (result && failureOf(result)) || couldntReady();
			patch(target, (file) => ({ ...file, state: "failed", error }));
		};

		const fail = (target: string, error: unknown) =>
			patch(target, (file) => ({
				...file,
				state: "failed",
				error: error instanceof Error && error.message ? error.message : couldntReady(),
			}));

		const stageFound = (target: string, found: ClipboardFound) =>
			source.stageFound(found.agent, [found.path]).then(
				(results) => settle(target, results[0]),
				(error: unknown) => fail(target, error)
			);

		const add = (file: StagedFile) => set({ files: [...get().files, file] });

		/** Uploads a picked file to the agent, failing it once the message would carry too much. */
		const upload = (target: string, path: string) => {
			const onProgress = (progress: TransferProgress) => {
				if (find(target)?.state !== "staging") {
					stop(progress.transfer);
					return;
				}
				const going: StagedUpload = {
					transfer: progress.transfer,
					sent: progress.sent,
					total: progress.total,
					state: progress.state,
				};
				const files = get().files.map((file) =>
					file.key === target ? { ...file, upload: going } : file
				);
				const over = overLimit(countedOf(files), target);
				if (over) stop(progress.transfer);
				set({
					files: over
						? files.map((file) =>
								file.key === target
									? { ...file, state: "failed", error: over }
									: file
							)
						: files,
				});
			};
			source.upload(device, { type: "agent", agent }, [path], onProgress).then(
				([result]) => {
					if (find(target)?.state === "staging") settle(target, result);
				},
				(error: unknown) => {
					if (find(target)?.state === "staging") fail(target, error);
				}
			);
		};

		return {
			files: [],
			addPicked: (picked, uploads) => {
				const path = uploads ? picked.path : null;
				if (path === null && tooLarge(picked.size)) return { refused: tooLargeText() };
				const target = key();
				const file: StagedFile = {
					key: target,
					name: picked.name,
					kind: kindOfMime(picked.mime),
					bytes: picked.size,
					preview: picked.preview,
					state: "staging",
					ref: null,
					error: null,
					found: null,
					upload:
						path === null
							? null
							: {
									transfer: null,
									sent: 0,
									total: picked.size ?? 0,
									state: "uploading",
								},
				};
				if (path !== null) {
					// The picker's size lets the limits turn a file away before any of it copies.
					const over = overLimit(countedOf([...get().files, file]), target);
					if (over) return { refused: over };
					add(file);
					upload(target, path);
					return { key: target };
				}
				add(file);
				picked
					.read()
					.then((bytes) => {
						if (!get().files.some((each) => each.key === target)) return undefined;
						if (tooLarge(bytes.length)) {
							patch(target, (each) => ({
								...each,
								state: "failed",
								error: tooLargeText(),
							}));
							return undefined;
						}
						return source
							.stagePasted(device, picked.name, picked.mime, bytes)
							.then((result) => settle(target, result));
					})
					.catch((error: unknown) => fail(target, error));
				return { key: target };
			},
			addFound: (agent, on, path) => {
				const target = key();
				const found: ClipboardFound = { agent, device: on, path };
				add({
					key: target,
					name: nameOfPath(path),
					kind: path.endsWith("/") ? "folder" : "file",
					bytes: null,
					preview: null,
					state: "staging",
					ref: null,
					error: null,
					found,
					upload: null,
				});
				void stageFound(target, found);
				return target;
			},
			addReady: (ref) => {
				const target = key();
				add({
					key: target,
					name: ref.name,
					kind: ref.kind,
					bytes: ref.bytes,
					preview: null,
					state: "ready",
					ref,
					error: null,
					found: null,
					upload: null,
				});
				return target;
			},
			restore: (files) => {
				const known = new Set(get().files.map((file) => file.key));
				const fresh = files.filter((file) => !known.has(file.key));
				set({
					files: [
						...get().files,
						...fresh.map((file): StagedFile => ({
							key: file.key,
							name: file.name,
							kind: file.kind,
							bytes: file.ref?.bytes ?? null,
							preview: null,
							state: file.ref ? "ready" : file.found ? "staging" : "failed",
							ref: file.ref,
							error:
								file.ref || file.found ? null : t("No longer ready. Add it again."),
							found: file.found ?? null,
							upload: null,
						})),
					],
				});
				for (const file of fresh)
					if (!file.ref && file.found) void stageFound(file.key, file.found);
			},
			remove: (target) => {
				stopAll(get().files.filter((file) => file.key === target));
				set({ files: get().files.filter((file) => file.key !== target) });
			},
			keepOnly: (keys) => {
				const files = get().files.filter((file) => keys.has(file.key));
				if (files.length === get().files.length) return;
				stopAll(get().files.filter((file) => !keys.has(file.key)));
				set({ files });
			},
			clear: () => {
				stopAll(get().files);
				set({ files: [] });
			},
		};
	});

/** Whether every file is ready to send. */
export const allReady = (files: readonly StagedFile[]): boolean =>
	files.every((file) => file.state === "ready");

/** Waits until no file is still staging, and gives the files as they then are. */
export const settled = (staging: Staging): Promise<readonly StagedFile[]> =>
	new Promise((resolve) => {
		const done = (files: readonly StagedFile[]) =>
			!files.some((file) => file.state === "staging");
		if (done(staging.getState().files)) {
			resolve(staging.getState().files);
			return;
		}
		const stop = staging.subscribe(({ files }) => {
			if (!done(files)) return;
			stop();
			resolve(files);
		});
	});
