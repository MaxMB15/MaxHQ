// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A store per open timeline. Resets and older pages apply as they arrive.
// Events wait for the next frame and apply together, so a reply streaming
// fifty chunks a second still renders once a frame.

import { t } from "../i18n";

import { createStore, type StoreApi } from "zustand/vanilla";
import type { AgentId, CommandError, TimelineItem, TimelineUpdate } from "../bindings";

import type { DataSource, Unsubscribe } from "../data/source";
import { applyUpdate, EMPTY_TIMELINE, type TimelineState } from "./apply";

/** A store holding one agent's timeline. */
export type TimelineStore = StoreApi<TimelineState>;

/** Runs `flush` soon, once. */
export type Schedule = (flush: () => void) => void;

/** An open timeline: its store and what you can ask of it. */
export type Timeline = {
	readonly store: TimelineStore;
	/** Asks for the page before the first item here, unless one is on its way or there's none. */
	readonly loadOlder: () => void;
	/**
	 * Opens the timeline again, which brings a reset with the newest page.
	 * The open before keeps going until the new one is in, and stays if it
	 * fails.
	 */
	readonly reload: () => Promise<void>;
	/**
	 * Calls `listener` with the items that just appeared while the timeline
	 * was open: new items in live changes, never ones from a reset, a page,
	 * or what an open or a reconnect caught up on.
	 */
	readonly onArrive: (listener: (items: readonly TimelineItem[]) => void) => Unsubscribe;
	/** Stops listening. */
	readonly close: Unsubscribe;
};

const nextFrame: Schedule = (flush) => {
	if (typeof requestAnimationFrame === "function") requestAnimationFrame(flush);
	else setTimeout(flush, 16);
};

const messageOf = (error: unknown): string => {
	if (error instanceof Error) return error.message;
	if (typeof error === "string") return error;
	return JSON.stringify(error);
};

/** A command's error, for a person to read. */
export const errorText = (error: CommandError): string => {
	switch (error.kind) {
		case "not_found":
			return t("Couldn't find {{error_what}}.", { error_what: error.what });
		case "not_connected":
			return t("{{error_device}} isn't connected.", { error_device: error.device });
		case "failed":
			return error.message;
	}
};

/** An empty store that waits for `openTimeline`. */
export const createTimelineStore = (): TimelineStore =>
	createStore<TimelineState>(() => EMPTY_TIMELINE);

/** Opens an agent's timeline from the source and keeps the store current. */
export const openTimeline = (
	source: DataSource,
	agent: AgentId,
	options: { readonly store?: TimelineStore; readonly schedule?: Schedule } = {}
): Timeline => {
	const store = options.store ?? createTimelineStore();
	const schedule = options.schedule ?? nextFrame;
	let stopped = false;
	let unsubscribe: Unsubscribe | null = null;
	let waiting: { readonly update: TimelineUpdate; readonly live: boolean }[] = [];
	let scheduled = false;
	const arrivals = new Set<(items: readonly TimelineItem[]) => void>();

	const flush = () => {
		scheduled = false;
		if (stopped || waiting.length === 0) return;
		let state = store.getState();
		const arrived: TimelineItem[] = [];
		for (const { update, live } of waiting) {
			const before = state;
			state = applyUpdate(state, update);
			if (!live || update.type !== "events" || state === before) continue;
			for (const event of update.events) {
				if (event.type === "upsert" && !before.items.has(event.item.id))
					arrived.push(event.item);
			}
		}
		waiting = [];
		if (state !== store.getState()) store.setState(state, true);
		if (arrived.length > 0) for (const listener of [...arrivals]) listener(arrived);
	};

	const onUpdate = (update: TimelineUpdate, live: boolean) => {
		if (stopped) return;
		waiting.push({ update, live });
		if (update.type !== "events") flush();
		else if (!scheduled) {
			scheduled = true;
			schedule(flush);
		}
	};

	const fail = (error: unknown) => {
		if (!stopped) store.setState({ status: "failed", error: messageOf(error) });
	};

	// A later open takes over from the one before, which closes once the
	// new one is in.
	const open = () =>
		source.openTimeline(agent, onUpdate).then((stop) => {
			if (stopped) return stop();
			const before = unsubscribe;
			unsubscribe = stop;
			before?.();
		});
	open().catch(fail);

	const loadOlder = () => {
		const state = store.getState();
		if (stopped || state.status !== "ready" || !state.hasOlder || state.loadingOlder) return;
		const first = state.order[0] ? state.items.get(state.order[0]) : undefined;
		if (!first) return;
		store.setState({ loadingOlder: true });
		source.loadOlder(agent, first.ordinal).then(
			(result) => {
				if (stopped || result.status === "ok") return;
				store.setState({ loadingOlder: false, error: errorText(result.error) });
			},
			(error: unknown) => {
				if (!stopped) store.setState({ loadingOlder: false, error: messageOf(error) });
			}
		);
	};

	return {
		store,
		loadOlder,
		reload: () => (stopped ? Promise.resolve() : open().catch(() => {})),
		onArrive: (listener) => {
			arrivals.add(listener);
			return () => void arrivals.delete(listener);
		},
		close: () => {
			stopped = true;
			unsubscribe?.();
		},
	};
};
