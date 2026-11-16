// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The files a phone's box holds, each on its way to the agent's device.
// A photo or a file you pick goes as its bytes, up to the 8 MiB maxd takes
// in one message, and one over that stays out. A file the `@` menu found
// stages from the agent's own folder. A draft that comes back brings its
// files' staged copies with it while maxd still keeps them, and stages a
// found file again once its copy has gone.

import { createStore, type StoreApi } from "zustand/vanilla";

import type { AgentId, AttachmentKind, AttachmentRef, DeviceId } from "../bindings";
import { MAX_INLINE_ATTACHMENT } from "../bindings";
import { failureOf } from "../data/failure";
import type { ActionResult, DataSource } from "../data/source";
import { t } from "../i18n";
import type { ClipboardFile, ClipboardFound } from "../pills/clipboard";
import type { FileFacts } from "../pills/draft";

/** The most a phone sends of one file, as maxd takes it in one message. */
export const MAX_PHONE_FILE = MAX_INLINE_ATTACHMENT;

/** What a file too big for the phone to send says. */
export const tooLargeText = (): string =>
	t("Too large to send from the phone yet (up to {{size}} MB)", {
		size: MAX_PHONE_FILE / (1024 * 1024),
	});

/** Whether a file of `size` bytes is too big to send from the phone. */
export const tooLarge = (size: number | null | undefined): boolean =>
	size !== null && size !== undefined && size > MAX_PHONE_FILE;

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
};

/** A photo or file picked on the phone, with a way to read its bytes. */
export type PickedFile = {
	readonly name: string;
	readonly mime: string;
	/** Its size, when the picker knows it. */
	readonly size: number | null;
	readonly preview: string | null;
	readonly read: () => Promise<Uint8Array>;
};

export type StagingState = {
	readonly files: readonly StagedFile[];
	/**
	 * Stages a picked file as bytes. Returns its key, or why it stays out,
	 * such as a file over the limit.
	 */
	readonly addPicked: (file: PickedFile) => { key: string } | { refused: string };
	/** Stages a file or folder the `@` menu found in an agent's folder. Returns its key. */
	readonly addFound: (agent: AgentId, device: DeviceId, path: string) => string;
	/** Adds a ready file, such as one a queued message already carries. */
	readonly addReady: (ref: AttachmentRef) => string;
	/** Brings back a saved draft's files, staging a found one again when its copy has gone. */
	readonly restore: (files: readonly ClipboardFile[]) => void;
	/** Takes a file out. */
	readonly remove: (key: string) => void;
	/** Keeps only these files, as after the text lost a pill. */
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

/** Files for an agent on `device`, staged through `source`. */
export const createStaging = (
	source: Pick<DataSource, "stagePasted" | "stageFound">,
	device: DeviceId,
	key: () => string = newKey
): Staging =>
	createStore<StagingState>()((set, get) => {
		const patch = (target: string, change: (file: StagedFile) => StagedFile) =>
			set({
				files: get().files.map((file) => (file.key === target ? change(file) : file)),
			});

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

		return {
			files: [],
			addPicked: (picked) => {
				if (tooLarge(picked.size)) return { refused: tooLargeText() };
				const target = key();
				add({
					key: target,
					name: picked.name,
					kind: kindOfMime(picked.mime),
					bytes: picked.size,
					preview: picked.preview,
					state: "staging",
					ref: null,
					error: null,
					found: null,
				});
				picked
					.read()
					.then((bytes) => {
						if (!get().files.some((file) => file.key === target)) return undefined;
						if (tooLarge(bytes.length)) {
							patch(target, (file) => ({
								...file,
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
						})),
					],
				});
				for (const file of fresh)
					if (!file.ref && file.found) void stageFound(file.key, file.found);
			},
			remove: (target) => set({ files: get().files.filter((file) => file.key !== target) }),
			keepOnly: (keys) => {
				const files = get().files.filter((file) => keys.has(file.key));
				if (files.length !== get().files.length) set({ files });
			},
			clear: () => set({ files: [] }),
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
