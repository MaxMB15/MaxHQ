// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pictures and videos the timeline shows from an agent's device. Each one
// comes through the source into the app's media cache, the same way for an
// agent here, on another device or in a container, so the device's limits
// on what an agent can read are the only gate. This store keeps where each
// stands by agent and path, so rows showing the same file share one fetch,
// and a row that scrolls away and back finds it done. A picture up to
// PICTURE_LIMIT comes on sight, and a larger one waits for a click, showing
// its size. A video isn't asked for at all until a click, since even asking
// for its size makes the device read the whole file. Pictures on sight come
// PICTURES_AT_ONCE at a time and the rest wait their turn, so a reply that
// names many can't make a client pull them all from the device at once. A
// click doesn't wait, and neither does a file pane. A file outside the
// agent's folders comes only with Show anyway, which the file pane asks for
// on a click, and keeps its own entry so a fetch on sight never finds it. A
// picture the phone's cache let go of while the app was away comes again
// once on its own.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { AgentId, TransferId } from "../bindings";
import type { DataSource, MediaResult } from "../data/source";
import { errorText } from "../timeline/store";

/** The largest picture fetched when it scrolls into view. */
export const PICTURE_LIMIT = 16 * 1024 * 1024;

/** The largest file fetched at all, once you ask for it. The app's cache takes no more. */
export const FULL_LIMIT = 1024 * 1024 * 1024;

/** How many pictures come on sight at once. The rest wait their turn. */
export const PICTURES_AT_ONCE = 4;

/** Where one picture or video stands. */
export type MediaState =
	| {
			readonly state: "loading";
			readonly sent: number;
			/** Bytes in all, or 0 until the device has said. */
			readonly total: number;
	  }
	| { readonly state: "ready"; readonly url: string; readonly size: number }
	| {
			/** Larger than the limit it was asked with, so it waits on the device for a click. */
			readonly state: "large";
			readonly size: number;
	  }
	| {
			readonly state: "failed";
			readonly reason: string;
			/**
			 * Set when the device refused the path: "outside" when it's outside
			 * the agent's folders, "path" when it's bad some other way. A
			 * `maxd` from before Show anyway says "path" for both.
			 */
			readonly refused?: "outside" | "path";
	  };

const store = createStore<{ media: ReadonlyMap<string, MediaState> }>(() => ({
	media: new Map(),
}));

const keyFor = (agent: AgentId, path: string, anyway = false) =>
	anyway ? `${agent}\n${path}\nanyway` : `${agent}\n${path}`;

const put = (key: string, state: MediaState | null) =>
	store.setState(({ media }) => {
		const next = new Map(media);
		if (state) next.set(key, state);
		else next.delete(key);
		return { media: next };
	});

// A fetch on its way, or waiting its turn while `go` is set. Cancelling
// marks it, so the progress and the result that still come in change nothing.
type Job = {
	transfer: TransferId | null;
	cancelled: boolean;
	size: number | null;
	limit: number;
	go: (() => void) | null;
};

const jobs = new Map<string, Job>();

// The pictures fetched again because the cache let go of them, so a second
// miss fails instead of fetching again.
const refetched = new Set<string>();

// Fetches on sight under way, and the ones waiting their turn, oldest first.
const onSight = new Set<Job>();
const waiting: Job[] = [];

// Null when a fetch on sight may start now, or what settles once it may.
const room = (job: Job): Promise<void> | null => {
	if (onSight.size < PICTURES_AT_ONCE) {
		onSight.add(job);
		return null;
	}
	return new Promise((resolve) => {
		job.go = resolve;
		waiting.push(job);
	});
};

const release = (job: Job) => {
	const { go } = job;
	job.go = null;
	go?.();
};

// Lets a waiting fetch go out of turn, for a click or a cancel.
const unqueue = (job: Job) => {
	const at = waiting.indexOf(job);
	if (at >= 0) waiting.splice(at, 1);
	release(job);
};

// A fetch on sight is over, so the oldest one waiting takes its place.
const done = (job: Job) => {
	if (!onSight.delete(job)) return;
	const next = waiting.shift();
	if (!next) return;
	onSight.add(next);
	release(next);
};

/**
 * Where a picture or video stands, or null before anything asked for it.
 * `anyway` asks after the Show anyway fetch, which is kept apart.
 */
export const mediaState = (agent: AgentId, path: string, anyway = false): MediaState | null =>
	store.getState().media.get(keyFor(agent, path, anyway)) ?? null;

/**
 * Fetches a picture or video an agent can see into the media cache, unless
 * it's larger than `limit` bytes. Does nothing while it's on its way, once
 * it's here, or when it's known to be larger than `limit`. One asked for on
 * sight, with PICTURE_LIMIT, waits its turn behind PICTURES_AT_ONCE others.
 * `anyway` asks for one file outside the agent's folders, and only a click
 * on Show anyway may set it, so it never waits.
 */
export const loadMedia = async (
	source: DataSource,
	agent: AgentId,
	path: string,
	limit: number,
	anyway = false
): Promise<void> => {
	// Only a fetch on sight asks with PICTURE_LIMIT. A click asks with more,
	// the file pane with its own limit, and Show anyway with `anyway`, and
	// none of those wait their turn.
	const sight = limit === PICTURE_LIMIT && !anyway;
	const key = keyFor(agent, path, anyway);
	const running = jobs.get(key);
	if (running) {
		// A click on a picture still waiting its turn fetches it now.
		if (running.go && !sight) {
			running.limit = Math.max(running.limit, limit);
			unqueue(running);
		}
		return;
	}
	const now = mediaState(agent, path, anyway);
	if (now?.state === "ready" || (now?.state === "large" && now.size > limit)) return;
	const job: Job = {
		transfer: null,
		cancelled: false,
		size: now?.state === "large" ? now.size : null,
		limit,
		go: null,
	};
	jobs.set(key, job);
	put(key, { state: "loading", sent: 0, total: job.size ?? 0 });
	const turn = sight ? room(job) : null;
	if (turn) {
		await turn;
		if (job.cancelled) {
			done(job);
			return;
		}
	}
	let result: MediaResult;
	try {
		result = await source.media(agent, path, job.limit, anyway, (progress) => {
			const first = job.transfer === null;
			job.transfer = progress.transfer;
			// Cancelled before the transfer had a name, so it stops now.
			if (job.cancelled) {
				if (first) void source.cancelTransfer(progress.transfer);
				return;
			}
			if (progress.total > 0) job.size = progress.total;
			put(key, { state: "loading", sent: progress.sent, total: progress.total });
		});
	} catch (error) {
		result = {
			status: "error",
			error: {
				kind: "failed",
				message: error instanceof Error ? error.message : String(error),
			},
		};
	}
	done(job);
	if (job.cancelled) return;
	jobs.delete(key);
	if (result.status === "error") {
		put(key, { state: "failed", reason: errorText(result.error) });
	} else if (result.data.kind === "cached") {
		put(key, { state: "ready", url: result.data.url, size: result.data.size });
	} else if (result.data.kind === "refused") {
		put(key, {
			state: "failed",
			reason: sentence(result.data.message),
			refused: result.data.outside ? "outside" : "path",
		});
	} else {
		put(key, { state: "large", size: result.data.size });
	}
};

// A device's message as a sentence: capitalised, with a full stop.
const sentence = (message: string): string => {
	const text = message.trim();
	if (!text) return "";
	const first = text[0]!.toUpperCase() + text.slice(1);
	return /[.!?]$/.test(first) ? first : `${first}.`;
};

/** Stops a fetch on its way. One whose size is known waits for a click again. */
export const cancelMedia = (source: DataSource, agent: AgentId, path: string): void => {
	const key = keyFor(agent, path);
	const job = jobs.get(key);
	if (!job) return;
	job.cancelled = true;
	jobs.delete(key);
	unqueue(job);
	if (job.transfer) void source.cancelTransfer(job.transfer);
	put(key, job.size === null ? null : { state: "large", size: job.size });
};

/**
 * Marks a picture or video the webview couldn't show, such as one in a
 * format it doesn't play, or one the cache let go of since. Loading it
 * again fetches it again.
 */
export const mediaFailed = (agent: AgentId, path: string, reason: string): void =>
	put(keyFor(agent, path), { state: "failed", reason });

/**
 * Fetches again a picture the app couldn't show from the media cache, since
 * iOS may have emptied the cache while the app was away. That happens once
 * for each picture. The next time it's marked failed with `reason`, so a
 * picture that can't show never fetches over and over. One that isn't ready,
 * such as one another row already fetches again, is left alone.
 */
export const mediaLost = async (
	source: DataSource,
	agent: AgentId,
	path: string,
	reason: string
): Promise<void> => {
	const key = keyFor(agent, path);
	const now = mediaState(agent, path);
	if (now?.state !== "ready") return;
	if (refetched.has(key)) {
		mediaFailed(agent, path, reason);
		return;
	}
	refetched.add(key);
	put(key, null);
	// One larger than a fetch on sight takes was asked for, so it comes now.
	await loadMedia(source, agent, path, now.size > PICTURE_LIMIT ? FULL_LIMIT : PICTURE_LIMIT);
};

/**
 * Where a picture or video stands, or null before anything asked for it.
 * `anyway` follows the Show anyway fetch instead.
 */
export const useMedia = (agent: AgentId, path: string, anyway = false): MediaState | null =>
	useStore(store, (s) => s.media.get(keyFor(agent, path, anyway)) ?? null);

/** Forgets every picture and video, for tests. */
export const clearMedia = (): void => {
	for (const job of jobs.values()) job.cancelled = true;
	jobs.clear();
	onSight.clear();
	for (const job of waiting.splice(0)) release(job);
	refetched.clear();
	store.setState({ media: new Map() });
};
