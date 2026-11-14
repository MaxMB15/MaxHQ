// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The drafts that outlive the app: what you typed to each agent and haven't
// sent, kept on this device so a restart brings it back. A draft keeps its
// text, its pills and its files' staged references, never the files
// themselves. A file comes back as the copy maxd staged, which it keeps for
// a day. After that, or when it hadn't staged yet, it comes back as its
// marker's words, since the app won't read a path from an earlier run
// until you hand the file over again. Typing writes at most every few
// hundred milliseconds, while a draft that's sent, emptied or for an agent
// that's gone goes at once. Past a size cap the oldest drafts go first,
// and one left alone for 30 days goes too. Each write reads the store
// again first, so windows that share it keep each other's drafts.

import type { AgentId, AttachmentId } from "../bindings";
import { type ClipboardFile, type ClipboardFragment, isFragment } from "../pills/clipboard";
import { isBlank, plainText } from "../pills/draft";
import type { ViewState } from "../view/replica";
import type { ViewStore } from "../view/store";

/** Where a platform keeps the drafts: one string, read and written whole. */
export type DraftStorage = {
	readonly read: () => string | null;
	readonly write: (text: string) => void;
};

/** How long a draft left alone stays. */
export const SAVED_FOR_MS = 30 * 24 * 60 * 60 * 1000;

/**
 * How long a staged file's reference is used again. maxd deletes a staged
 * copy nobody sent after a day, and this leaves an hour to send it.
 */
export const STAGED_FOR_MS = 23 * 60 * 60 * 1000;

/** The most the drafts take together, in characters of their JSON. */
export const MAX_SAVED_CHARS = 1_000_000;

/** How long typing waits before it writes. */
const WRITE_DELAY_MS = 300;

/** A draft and when it was last saved. */
type Saved = { readonly at: number; readonly fragment: ClipboardFragment };

/** The drafts kept on this device. */
export type SavedDrafts = {
	/** The draft saved for an agent, or null when there's none. */
	readonly get: (agent: AgentId) => ClipboardFragment | null;
	/** Saves an agent's draft soon, or drops it at once when it's null or blank. */
	readonly save: (agent: AgentId, fragment: ClipboardFragment | null) => void;
	/** Drops the agents' drafts at once. */
	readonly drop: (agents: readonly AgentId[]) => void;
	/** Writes what waits now, such as when the app goes away. */
	readonly flush: () => void;
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
	typeof value === "object" && value !== null && !Array.isArray(value);

/** The drafts in the stored text. A garbled store, or a draft that isn't one, reads as none. */
const parse = (text: string | null): Map<AgentId, Saved> => {
	const drafts = new Map<AgentId, Saved>();
	if (!text) return drafts;
	try {
		const value: unknown = JSON.parse(text);
		if (!isRecord(value) || value.version !== 1 || !isRecord(value.drafts)) return drafts;
		for (const [agent, saved] of Object.entries(value.drafts)) {
			if (isRecord(saved) && Number.isFinite(saved.at) && isFragment(saved.fragment))
				drafts.set(agent, { at: saved.at as number, fragment: saved.fragment });
		}
	} catch {
		// Not JSON.
	}
	return drafts;
};

/**
 * The drafts that stay, newest first: the ones saved in the last 30 days
 * that fit the cap.
 */
const trimmed = (drafts: ReadonlyMap<AgentId, Saved>, now: number): Map<AgentId, Saved> => {
	const kept = new Map<AgentId, Saved>();
	let size = 0;
	for (const [agent, saved] of [...drafts].sort(([, a], [, b]) => b.at - a.at)) {
		if (now - saved.at > SAVED_FOR_MS) break;
		const chars = agent.length + JSON.stringify(saved).length;
		if (size + chars > MAX_SAVED_CHARS) continue;
		size += chars;
		kept.set(agent, saved);
	}
	return kept;
};

/**
 * When maxd staged a file, from the time its ID carries, or null for an ID
 * that carries none. maxd gives each staged copy a UUIDv7.
 */
const stagedAt = (id: AttachmentId): number | null => {
	const hex = id.replaceAll("-", "").toLowerCase();
	return /^[0-9a-f]{12}7[0-9a-f]{19}$/.test(hex) ? parseInt(hex.slice(0, 12), 16) : null;
};

/**
 * The file as it can come back: as it was while maxd still holds its
 * staged copy, and otherwise without the copy or its path, so it shows as
 * its marker. A file an agent has still stages again from that agent.
 */
const restorable = (file: ClipboardFile, now: number): ClipboardFile => {
	const at = file.ref === null ? null : stagedAt(file.ref.id);
	return at !== null && now - at < STAGED_FOR_MS
		? file
		: { ...file, ref: null, device: null, source: null };
};

/** Drafts kept in `storage`. */
export const createSavedDrafts = (
	storage: DraftStorage,
	{ now = Date.now, delay = WRITE_DELAY_MS }: { now?: () => number; delay?: number } = {}
): SavedDrafts => {
	// The drafts not written yet, with null for one to drop.
	const pending = new Map<AgentId, Saved | null>();
	// The agents with a draft when the store was last read or written, so
	// dropping one that has none needn't read it. Null until the first read.
	let held: ReadonlySet<AgentId> | null = null;
	let timer: ReturnType<typeof setTimeout> | null = null;

	const read = () => {
		let text: string | null = null;
		try {
			text = storage.read();
		} catch {
			// Unreadable storage holds no drafts.
		}
		const drafts = parse(text);
		held = new Set(drafts.keys());
		return drafts;
	};

	const flush = () => {
		if (timer !== null) clearTimeout(timer);
		timer = null;
		if (pending.size === 0) return;
		const drafts = read();
		for (const [agent, saved] of pending) {
			if (saved) drafts.set(agent, saved);
			else drafts.delete(agent);
		}
		pending.clear();
		const kept = trimmed(drafts, now());
		held = new Set(kept.keys());
		try {
			storage.write(JSON.stringify({ version: 1, drafts: Object.fromEntries(kept) }));
		} catch {
			// Full or gone, so the drafts last only while the app runs.
		}
	};

	const drop = (agents: readonly AgentId[]) => {
		if (held === null) read();
		const stored = held ?? new Set<AgentId>();
		const known = agents.filter((agent) => pending.has(agent) || stored.has(agent));
		if (known.length === 0) return;
		for (const agent of known) pending.set(agent, null);
		flush();
	};

	return {
		get: (agent) => {
			const saved = pending.has(agent) ? pending.get(agent) : read().get(agent);
			const at = now();
			if (!saved || at - saved.at > SAVED_FOR_MS) return null;
			const { fragment } = saved;
			return { ...fragment, files: fragment.files.map((file) => restorable(file, at)) };
		},
		save: (agent, fragment) => {
			if (fragment === null || isBlank(fragment)) {
				drop([agent]);
				return;
			}
			pending.set(agent, { at: now(), fragment });
			timer ??= setTimeout(flush, delay);
		},
		drop,
		flush,
	};
};

/** The agents `before` held that `after` doesn't, once both have loaded. */
export const goneAgents = (before: ViewState, after: ViewState): AgentId[] => {
	const was = before.tables.agent;
	const is = after.tables.agent;
	if (before.seq === null || after.seq === null || was === is) return [];
	return [...was.keys()].filter((agent) => !is.has(agent));
};

/** Drops an agent's draft once the view loses the agent. Returns the way to stop. */
export const dropGoneDrafts = (view: ViewStore, drafts: SavedDrafts): (() => void) =>
	view.subscribe((state, previous) => {
		const gone = goneAgents(previous.view, state.view);
		if (gone.length > 0) drafts.drop(gone);
	});

/** A draft of plain text, as the phone's box holds one. */
export const textDraft = (text: string): ClipboardFragment => ({
	version: 1,
	origin: "",
	segments: [{ type: "text", text }],
	files: [],
	gitItems: [],
});

/** A draft as plain text, each pill as its marker, or "" for none. */
export const draftText = (fragment: ClipboardFragment | null): string => {
	if (!fragment) return "";
	const files = new Map(fragment.files.map((file) => [file.key, file]));
	return plainText(fragment.segments, (key) => files.get(key));
};
