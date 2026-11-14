// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Saving an open note as you type. A save goes a second after you stop, at
// most ten seconds apart while you keep going, and right away when a field
// loses focus, the pane closes or the window goes. Each save names the
// version it was edited from, so a device that moved on merges the two, and
// the merged text comes back into the editor around what you typed since.
// A newer head from elsewhere comes in the same way while nothing waits to
// be saved. A refused save keeps your text and says why, and the next
// change or blur tries again.

import { t } from "../i18n";

import type { NoteVersion, VersionHash } from "../bindings";
import type { NoteSave, SaveNoteResult } from "../data/source";
import { errorText } from "../timeline/store";
import { applyReplacements, replacementsBetween } from "./diff";
import { mergeText } from "./merge";

/** How long after the last change a save goes. */
export const IDLE_MS = 1_000;
/** The longest a change waits while typing goes on. */
export const MAX_WAIT_MS = 10_000;

/** Where saving stands, for the pane's header. */
export type SaveState =
	| { readonly kind: "saved" }
	| { readonly kind: "saving" }
	| { readonly kind: "failed"; readonly reason: string };

/** Where saving stands, in words, or "Read-only" for a note that can't be changed here. */
export const saveText = (save: SaveState, readOnly: boolean): string => {
	if (readOnly) return t("Read-only");
	if (save.kind === "saved") return t("Saved");
	if (save.kind === "saving") return t("Saving");
	return t("Not saved: {{save_reason}}", { save_reason: save.reason });
};

/** What the pane shows of a note being edited. */
export type NoteSnapshot = {
	/** The title in the field. */
	readonly title: string;
	readonly save: SaveState;
	/** Whether there's a change the device doesn't have yet. */
	readonly unsaved: boolean;
	/** Whether the note's head is deleted. */
	readonly deleted: boolean;
	/** The version the text was last loaded or saved as. */
	readonly base: VersionHash;
};

export type AutosaveOptions = {
	/** The version the pane opened. */
	readonly initial: NoteVersion;
	readonly save: (save: NoteSave) => Promise<SaveNoteResult>;
	/** Reads a version from the device, or null when it can't. */
	readonly read: (version: VersionHash) => Promise<NoteVersion | null>;
	readonly idleMs?: number;
	readonly maxWaitMs?: number;
};

/** One open note's saving. */
export type Autosave = {
	/** Takes a change from the title field or the editor. */
	readonly edit: (change: { readonly title?: string; readonly body?: string }) => void;
	/** Saves what waits now, and resolves once the device answered. */
	readonly flush: () => Promise<void>;
	/** Tells it the device's head is now `head`. */
	readonly headMoved: (head: VersionHash) => void;
	/**
	 * Saves a new version after what waits: another title or body, such as
	 * an old version's, or the note deleted or back. What's left out stays
	 * as it is, and `deleted` is false unless given. Resolves with why it
	 * failed, or null.
	 */
	readonly replace: (next: {
		readonly title?: string;
		readonly body?: string;
		readonly deleted?: boolean;
	}) => Promise<string | null>;
	readonly snapshot: () => NoteSnapshot;
	readonly subscribe: (listener: () => void) => () => void;
	/**
	 * Hands each text from elsewhere, `body` made from `against`, to
	 * `into`, the editor, until the call it gives back.
	 */
	readonly follow: (into: (body: string, against: string) => void) => () => void;
};

/** Starts saving one open note. */
export const createAutosave = ({
	initial,
	save,
	read,
	idleMs = IDLE_MS,
	maxWaitMs = MAX_WAIT_MS,
}: AutosaveOptions): Autosave => {
	let known = initial;
	let draft = { title: initial.title, body: initial.body };
	let sending: Promise<void> | null = null;
	let again = false;
	let failed: string | null = null;
	let idle: ReturnType<typeof setTimeout> | undefined;
	let longest: ReturnType<typeof setTimeout> | undefined;
	const listeners = new Set<() => void>();
	let incoming: ((body: string, against: string) => void) | null = null;

	const unsaved = () => draft.title !== known.title || draft.body !== known.body;
	const shot = (): NoteSnapshot => {
		const waiting = unsaved();
		return {
			title: draft.title,
			save:
				failed !== null && waiting && !sending
					? { kind: "failed", reason: failed }
					: sending || waiting
						? { kind: "saving" }
						: { kind: "saved" },
			unsaved: waiting,
			deleted: known.deleted,
			base: known.hash,
		};
	};
	let current = shot();
	const changed = () => {
		current = shot();
		for (const listener of [...listeners]) listener();
	};
	const stopTimers = () => {
		clearTimeout(idle);
		clearTimeout(longest);
		idle = undefined;
		longest = undefined;
	};
	const schedule = () => {
		clearTimeout(idle);
		idle = setTimeout(() => void flush(), idleMs);
		longest ??= setTimeout(() => void flush(), maxWaitMs);
	};

	// Takes what the device kept from a save of `sent`.
	const took = (version: NoteVersion, sent: { title: string; body: string }) => {
		const typedSince = draft.body !== sent.body;
		known = version;
		failed = null;
		// A title changed here since stays, and a merged one shows otherwise.
		if (draft.title === sent.title) draft = { ...draft, title: version.title };
		if (version.body === sent.body) return;
		if (!typedSince) draft = { ...draft, body: version.body };
		// The editor puts in what the device merged around what was typed
		// since, and hands the result back as a change when the two differ.
		incoming?.(version.body, sent.body);
	};

	const send = async (): Promise<void> => {
		const sent = { ...draft };
		const result = await save({
			base: known.hash,
			title: sent.title,
			body: sent.body,
			deleted: false,
		});
		if (result.status === "error") failed = errorText(result.error);
		else took(result.data.version, sent);
	};

	const flush = (): Promise<void> => {
		if (sending) {
			again = true;
			return sending;
		}
		stopTimers();
		if (!unsaved()) return Promise.resolve();
		const run = send()
			.catch((error: unknown) => {
				failed = error instanceof Error ? error.message : String(error);
			})
			.finally(() => {
				sending = null;
				const asked = again;
				again = false;
				changed();
				// A flush asked for while this one went goes now. A change
				// made meanwhile waits its turn, unless the device refused.
				if (unsaved() && asked) void flush();
				else if (unsaved() && failed === null) schedule();
			});
		sending = run;
		changed();
		return run;
	};

	return {
		edit: (change) => {
			draft = { ...draft, ...change };
			if (unsaved()) schedule();
			changed();
		},
		flush,
		headMoved: (head) => {
			if (head === known.hash || sending || unsaved()) return;
			void read(head).then((version) => {
				// Typing or a save since then merges on the device instead.
				if (!version || sending || unsaved() || version.hash === known.hash) return;
				const against = known.body;
				known = version;
				draft = { title: version.title, body: version.body };
				if (version.body !== against) incoming?.(version.body, against);
				changed();
			});
		},
		replace: async (next) => {
			await flush();
			const result = await save({
				base: known.hash,
				title: next.title ?? draft.title,
				body: next.body ?? draft.body,
				deleted: next.deleted ?? false,
			});
			if (result.status === "error") {
				const reason = errorText(result.error);
				failed = reason;
				changed();
				return reason;
			}
			const against = draft.body;
			known = result.data.version;
			draft = { title: known.title, body: known.body };
			failed = null;
			if (known.body !== against) incoming?.(known.body, against);
			changed();
			return null;
		},
		snapshot: () => current,
		subscribe: (listener) => {
			listeners.add(listener);
			return () => {
				listeners.delete(listener);
			};
		},
		follow: (into) => {
			incoming = into;
			return () => {
				if (incoming === into) incoming = null;
			};
		},
	};
};

/**
 * What a plain text box holds once text from elsewhere comes in: `body`,
 * made from `against`, with what was typed in the box since `against` kept
 * around it. It's for a client whose editor can't place the change itself,
 * such as the phone's text box. Changes apart from each other both go in,
 * a character at a time. Where they touch the same text, the two merge a
 * line at a time, as the devices do, so nothing either side wrote is lost.
 * Hand the result back as an edit when it differs from `body`, so the next
 * save takes it.
 */
export const incomingText = (current: string, body: string, against: string): string => {
	if (current === against) return body;
	if (body === against) return current;
	const both = [
		...replacementsBetween(against, current),
		...replacementsBetween(against, body),
	].sort((a, b) => a.from - b.from || a.to - b.to);
	const touch = both.some((next, i) => {
		const prev = both[i - 1];
		return prev !== undefined && (next.from < prev.to || next.from === prev.from);
	});
	return touch ? mergeText(against, current, body) : applyReplacements(against, both);
};
