// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A store per open workflow run. A reset replaces it and each patch moves
// it one revision on. A patch that skips a revision means one went
// missing, so the run opens again, which starts it over with a reset.
// The shape and each node keep their identity until they change, across
// patches and resets alike, so a row subscribed to one node renders only
// when that node does.

import { createStore, type StoreApi } from "zustand/vanilla";
import type {
	LoopState,
	NodeState,
	RunHeader,
	RunKey,
	RunNotice,
	RunShape,
	WorkflowUpdate,
} from "../bindings";

import type { DataSource, Unsubscribe } from "../data/source";

/** Where one open run stands, as the views read it. */
export type RunState = {
	/** `loading` until the first reset, and `failed` when the run couldn't open. */
	readonly status: "loading" | "ready" | "failed";
	/** The revision the store is at, or null before the first reset. */
	readonly rev: number | null;
	readonly shape: RunShape | null;
	/** Each node's state, by id. */
	readonly nodes: ReadonlyMap<string, NodeState>;
	readonly loops: readonly LoopState[];
	readonly header: RunHeader | null;
	readonly notice: RunNotice | null;
	/** Why it failed to open, for a person to read. */
	readonly error: string | null;
};

export const EMPTY_RUN: RunState = {
	status: "loading",
	rev: null,
	shape: null,
	nodes: new Map(),
	loops: [],
	header: null,
	notice: null,
	error: null,
};

// A reset carries every node again, so one that comes back unchanged keeps
// its object. Both copies come from maxd's same serializer, so their JSON
// matches field for field.
const kept = (was: NodeState | undefined, now: NodeState): NodeState =>
	was !== undefined && JSON.stringify(was) === JSON.stringify(now) ? was : now;

/**
 * Applies an update. Returns null for a patch that doesn't follow the
 * revision the state is at, which needs a fresh reset.
 */
export const applyRunUpdate = (state: RunState, update: WorkflowUpdate): RunState | null => {
	if (update.kind === "reset") {
		const { rev, shape, nodes, loops, header, notice } = update.run;
		return {
			status: "ready",
			rev,
			// The same shape keeps its object, so the layout doesn't run again.
			shape: state.shape?.hash === shape.hash ? state.shape : shape,
			nodes: new Map(nodes.map((node) => [node.id, kept(state.nodes.get(node.id), node)])),
			loops,
			header,
			notice,
			error: null,
		};
	}
	const patch = update.run;
	if (state.rev === null || patch.rev !== state.rev + 1) return null;
	let nodes = state.nodes;
	if (patch.nodes.length > 0) {
		const next = new Map(nodes);
		for (const node of patch.nodes) next.set(node.id, node);
		nodes = next;
	}
	return {
		...state,
		rev: patch.rev,
		nodes,
		loops: patch.loops ?? state.loops,
		header: patch.header ?? state.header,
	};
};

/** A store holding one run. */
export type RunStore = StoreApi<RunState>;

/** An open run: its store and the call that stops it. */
export type OpenRun = {
	readonly store: RunStore;
	/** Stops listening. */
	readonly close: Unsubscribe;
};

const messageOf = (error: unknown): string =>
	error instanceof Error ? error.message : String(error);

/** Opens a run from the source and keeps its store current. */
export const openRun = (source: DataSource, run: RunKey): OpenRun => {
	const store = createStore<RunState>(() => EMPTY_RUN);
	let stopped = false;
	// Each open counts up, so updates for an open since replaced are dropped.
	let generation = 0;
	let unsubscribe: Unsubscribe | null = null;

	const open = () => {
		const mine = ++generation;
		const onUpdate = (update: WorkflowUpdate) => {
			if (stopped || mine !== generation) return;
			const next = applyRunUpdate(store.getState(), update);
			if (next) store.setState(next, true);
			else open();
		};
		source.openWorkflow(run, onUpdate).then(
			(stop) => {
				if (stopped || mine !== generation) {
					stop();
					return;
				}
				// The open before this one stays until this one is in, so the
				// run never closes on its device in between.
				unsubscribe?.();
				unsubscribe = stop;
			},
			(error: unknown) => {
				if (!stopped && mine === generation) {
					store.setState({ status: "failed", error: messageOf(error) });
				}
			}
		);
	};
	open();

	return {
		store,
		close: () => {
			stopped = true;
			unsubscribe?.();
			unsubscribe = null;
		},
	};
};
