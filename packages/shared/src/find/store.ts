// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: what the find bar
// holds, what it says about the matches, and when it asks again. Typing
// waits a moment before it asks. Next and previous move among the matches
// the last answer named, and ask only past them or once the timeline
// changed. A changing timeline asks again, at most every so often, and an
// answer to anything but the latest ask is dropped. The rest is MaxHQ's:
// each agent view holds its own find, maxd answers with the items near the
// active match instead of a list of all of them, and the desktop and the
// phone share it.

import { t } from "../i18n";

import { createStore, type StoreApi } from "zustand/vanilla";

import type { FindAt, FindMatch, TimelineFind } from "../bindings";
import { findQueryOf, nearItemAt, stepFindIndex } from "./search";

/** How long typing waits before it asks, in milliseconds. */
export const TYPE_MS = 100;
/** How long a changing timeline waits before it asks again, in milliseconds. */
export const REFRESH_MS = 300;

/** What an agent view's find holds. */
export type FindState = {
	readonly open: boolean;
	/** The query as typed. */
	readonly typed: string;
	/** Goes up each time the input should take focus and pick its text. */
	readonly focus: number;
	/** The query `result` answers, without its outer whitespace. */
	readonly query: string;
	/** The latest answer for the query, or null before one came. */
	readonly result: TimelineFind | null;
	/** The active match's index among all of them. */
	readonly index: number;
	/** The active match, or null when there's none. */
	readonly match: FindMatch | null;
	/** Set while an ask is out, and when the last one failed. */
	readonly status: "idle" | "searching" | "failed";
	/** Goes up each time find moves to a match, so the view goes there. */
	readonly turn: number;
};

/** A find whose bar is closed. */
export const CLOSED_FIND: FindState = {
	open: false,
	typed: "",
	focus: 0,
	query: "",
	result: null,
	index: 0,
	match: null,
	status: "idle",
	turn: 0,
};

/** What the bar says about the matches, and whether it's that there are none. */
export const findLabel = (state: FindState): { text: string; none: boolean } => {
	if (state.typed.trim().length === 0) return { text: "", none: false };
	if (state.status === "failed") return { text: t("Search failed"), none: false };
	if (state.result === null) return { text: t("Searching"), none: false };
	if (state.result.total === 0) return { text: t("No matches"), none: true };
	return {
		text: t("{{value1}} of {{value2}}", {
			value1: state.index + 1,
			value2: state.result.total,
		}),
		none: false,
	};
};

/** Asks maxd to find `query` across the timeline. */
export type FindAsk = (query: string, at: FindAt) => Promise<TimelineFind>;
/** The first match at or below the top of the view, if any row shows. */
export type FindPosition = (query: string) => FindMatch | undefined;

/** One agent view's find. */
export type Find = {
	readonly store: StoreApi<FindState>;
	/** Opens the bar, or focuses it and starts again from the view when it's open. */
	readonly open: () => void;
	/** Closes the bar and forgets the query. */
	readonly close: () => void;
	readonly type: (typed: string) => void;
	/** Moves `delta` matches on, wrapping round. */
	readonly step: (delta: number) => void;
	/** Says the timeline changed, so the counts may have. */
	readonly changed: () => void;
	/** Asks again after a failure. */
	readonly retry: () => void;
	/** Says where the view is, once the list mounts. Returns the undo. */
	readonly readFrom: (position: FindPosition) => () => void;
	/** Stops the timers and drops any answer still to come. */
	readonly dispose: () => void;
};

const sameMatch = (a: FindMatch | null, b: FindMatch | null) =>
	a === b || (a !== null && b !== null && a.item === b.item && a.occurrence === b.occurrence);

export const createFind = (ask: FindAsk): Find => {
	const store = createStore<FindState>(() => CLOSED_FIND);
	// Where the view is. Find starts from the first match before a list mounts.
	let position: FindPosition | null = null;
	// The latest ask, so an answer to an older one is dropped.
	let asked = 0;
	// The ask that's out, and whether its answer moves the view.
	let pending: { readonly at: FindAt; readonly moves: boolean } | null = null;
	// What the last ask was for, to ask again.
	let last: FindAt | null = null;
	// Set when the timeline changed since the last ask.
	let dirty = false;
	let typing: ReturnType<typeof setTimeout> | null = null;
	let refreshing: ReturnType<typeof setTimeout> | null = null;

	const stopTimers = () => {
		if (typing !== null) clearTimeout(typing);
		if (refreshing !== null) clearTimeout(refreshing);
		typing = null;
		refreshing = null;
	};

	const send = (query: string, at: FindAt, moves: boolean) => {
		const id = ++asked;
		pending = { at, moves };
		last = at;
		dirty = false;
		store.setState({ status: "searching" });
		ask(query, at).then(
			(found) => {
				if (id !== asked) return;
				pending = null;
				const state = store.getState();
				// A changed timeline that leaves the match where it was doesn't move the view.
				const turn =
					moves || !sameMatch(found.found, state.match) ? state.turn + 1 : state.turn;
				store.setState({
					query,
					result: found,
					index: found.active,
					match: found.found,
					status: "idle",
					turn,
				});
			},
			() => {
				if (id !== asked) return;
				pending = null;
				store.setState({ status: "failed" });
			}
		);
	};

	// Asks from where the view is, so the first match is the one you're reading.
	const fromView = (query: string) => {
		const from = position?.(query);
		send(query, from ? { type: "from", from, offset: 0 } : { type: "index", index: 0 }, true);
	};

	const refresh = () => {
		const state = store.getState();
		const query = findQueryOf(state.typed);
		if (!state.open || !query || typing !== null) return;
		if (pending) return send(query, pending.at, pending.moves);
		if (state.match) return send(query, { type: "from", from: state.match, offset: 0 }, false);
		send(query, last ?? { type: "index", index: 0 }, false);
	};

	return {
		store,
		open: () => {
			const state = store.getState();
			store.setState({ open: true, focus: state.focus + 1 });
			const query = findQueryOf(state.typed);
			if (state.open && query && typing === null) fromView(query);
		},
		close: () => {
			asked++;
			pending = null;
			last = null;
			dirty = false;
			stopTimers();
			const { focus, turn } = store.getState();
			store.setState({ ...CLOSED_FIND, focus, turn });
		},
		type: (typed) => {
			const state = store.getState();
			const query = findQueryOf(typed);
			const before = findQueryOf(state.typed);
			store.setState({ typed });
			if (query === before) return;
			asked++;
			pending = null;
			last = null;
			stopTimers();
			store.setState({
				query: "",
				result: null,
				index: 0,
				match: null,
				status: query ? "searching" : "idle",
			});
			if (!query) return;
			typing = setTimeout(() => {
				typing = null;
				fromView(query);
			}, TYPE_MS);
		},
		step: (delta) => {
			const state = store.getState();
			const { result, match } = state;
			const query = findQueryOf(state.typed);
			if (!state.open || !query || !result || result.total === 0 || typing !== null) return;
			// Steps taken while an ask is out add up on it.
			if (pending?.at.type === "from") {
				return send(query, { ...pending.at, offset: pending.at.offset + delta }, true);
			}
			const index = stepFindIndex(state.index, result.total, delta);
			const entry = dirty || pending ? undefined : nearItemAt(result.near, index);
			if (entry) {
				store.setState({
					index,
					match: { item: entry.item, occurrence: index - entry.first },
					turn: state.turn + 1,
				});
				return;
			}
			if (match) send(query, { type: "from", from: match, offset: delta }, true);
		},
		changed: () => {
			const state = store.getState();
			if (!state.open || !findQueryOf(state.typed)) return;
			dirty = true;
			if (refreshing !== null) return;
			refreshing = setTimeout(() => {
				refreshing = null;
				refresh();
			}, REFRESH_MS);
		},
		retry: () => {
			const query = findQueryOf(store.getState().typed);
			if (query && last) send(query, last, true);
		},
		readFrom: (read) => {
			position = read;
			return () => {
				if (position === read) position = null;
			};
		},
		dispose: () => {
			asked++;
			pending = null;
			stopTimers();
		},
	};
};
