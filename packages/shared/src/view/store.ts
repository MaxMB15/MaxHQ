// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Keeps the webview's copy of the view in step with a data source. It
// listens first and loads the snapshot second, so no change slips between
// the two, then applies the deltas that came in while the snapshot loaded.
// A gap in the versions loads the snapshot again. When listening or loading
// fails, it tries again on its own, waiting longer each time up to 30 s,
// and the status says when.

import { createStore, type StoreApi } from "zustand/vanilla";
import type { Delta, Timestamp } from "../bindings";

import type { DataSource, Unsubscribe } from "../data/source";
import { applyDelta, applySnapshot, EMPTY_VIEW, type ViewState } from "./replica";

/** Whether the view has loaded. */
export type ViewStatus =
	| { readonly state: "loading" }
	| { readonly state: "ready" }
	| {
			readonly state: "failed";
			readonly message: string;
			/** When the next try starts, or null while one runs. */
			readonly retryAt: Timestamp | null;
	  };

/** The view, whether it has loaded, and a way to try again. */
export type ViewStoreState = {
	readonly view: ViewState;
	readonly status: ViewStatus;
	/** Tries again now after a failure, without waiting. Does nothing otherwise. */
	readonly retry: () => void;
};

/** A store holding the view. */
export type ViewStore = StoreApi<ViewStoreState>;

const idle = () => {};

/** An empty store that waits for `connectView`. */
export const createViewStore = (): ViewStore =>
	createStore<ViewStoreState>(() => ({
		view: EMPTY_VIEW,
		status: { state: "loading" },
		retry: idle,
	}));

/** The longest wait between tries. */
export const RETRY_MAX_MS = 30_000;

/** How long to wait after `failures` failed tries in a row: 1 s, doubling each time, up to 30 s. */
export const retryDelay = (failures: number): number =>
	Math.min(RETRY_MAX_MS, 1000 * 2 ** Math.max(0, failures - 1));

const messageOf = (error: unknown): string => {
	if (error instanceof Error) return error.message;
	if (typeof error === "string") return error;
	return JSON.stringify(error);
};

/** Fills the store from the source and keeps it current. Returns a function that stops it. */
export const connectView = (store: ViewStore, source: DataSource): Unsubscribe => {
	let stopped = false;
	let unsubscribe: Unsubscribe | null = null;
	// Deltas that arrive while a snapshot loads. Null once it has loaded.
	let pending: Delta[] | null = [];
	// Failed tries in a row, and the wait before the next one.
	let failures = 0;
	let timer: ReturnType<typeof setTimeout> | null = null;

	const fail = (error: unknown) => {
		if (stopped) return;
		failures += 1;
		const wait = retryDelay(failures);
		if (timer !== null) clearTimeout(timer);
		timer = setTimeout(again, wait);
		store.setState({
			status: { state: "failed", message: messageOf(error), retryAt: Date.now() + wait },
		});
	};

	const load = async (): Promise<void> => {
		pending = [];
		let snapshot;
		try {
			snapshot = await source.snapshot();
		} catch (error) {
			fail(error);
			return;
		}
		if (stopped) return;
		let view = applySnapshot(snapshot);
		const waiting = pending;
		pending = null;
		for (const delta of waiting) {
			const result = applyDelta(view, delta);
			if (result.kind === "gap") return load();
			if (result.kind === "applied") view = result.view;
		}
		failures = 0;
		store.setState({ view, status: { state: "ready" } });
	};

	const onDelta = (delta: Delta) => {
		if (stopped) return;
		if (pending) {
			pending.push(delta);
			return;
		}
		const result = applyDelta(store.getState().view, delta);
		if (result.kind === "applied") store.setState({ view: result.view });
		else if (result.kind === "gap") void load();
	};

	const listen = () => {
		source.subscribe(onDelta).then((stop) => {
			if (stopped) {
				stop();
				return;
			}
			unsubscribe = stop;
			void load();
		}, fail);
	};

	// The next try listens first if that's what failed, and loads otherwise.
	// `fail` only runs it after a wait, once it's defined.
	const again = () => {
		timer = null;
		if (stopped) return;
		const { status } = store.getState();
		if (status.state === "failed") store.setState({ status: { ...status, retryAt: null } });
		if (unsubscribe) void load();
		else listen();
	};

	// Only a try that's waiting starts early, so a click never runs two at once.
	const retry = () => {
		if (timer === null) return;
		clearTimeout(timer);
		again();
	};

	store.setState({ retry });
	listen();

	return () => {
		stopped = true;
		if (timer !== null) clearTimeout(timer);
		if (store.getState().retry === retry) store.setState({ retry: idle });
		unsubscribe?.();
	};
};
