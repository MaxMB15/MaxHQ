// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Open workflow runs, shared by every view of the same run and held while
// a view is mounted, and the hooks that read one part of a run each. A row
// that reads one node renders only when that node changes.

import { useStore } from "zustand";

import type { LoopState, NodeState, RunHeader, RunKey, RunShape } from "../bindings";
import type { DataSource } from "../data/source";
import { sharedOpens } from "../data/shared-opens";
import { useSharedOpen } from "../data/use-shared-open";
import { runName } from "./select";
import { openRun, type OpenRun, type RunState } from "./store";

/** Every open run, by key. */
export const runOpens = sharedOpens<RunKey, OpenRun>(openRun, runName);

/** The run's store, open for as long as the calling view is mounted. */
export const useRun = (source: DataSource, run: RunKey): OpenRun =>
	useSharedOpen(runOpens, source, run);

/** The run's shape, which keeps its identity until its hash changes. */
export const useRunShape = (source: DataSource, run: RunKey): RunShape | null =>
	useStore(useRun(source, run).store, (state) => state.shape);

/** One node's state. */
export const useNode = (source: DataSource, run: RunKey, id: string): NodeState | undefined =>
	useStore(useRun(source, run).store, (state) => state.nodes.get(id));

/**
 * Every node's state, for a list that lays out by status. The map changes
 * with any node, and each node in it keeps its object until it changes.
 */
export const useRunNodes = (source: DataSource, run: RunKey): ReadonlyMap<string, NodeState> =>
	useStore(useRun(source, run).store, (state) => state.nodes);

/**
 * One value worked out from the run, such as whether a node was reached.
 * The view renders again only when the value changes, so `pick` should
 * return a primitive or an object the run keeps.
 */
export const useRunValue = <T>(source: DataSource, run: RunKey, pick: (state: RunState) => T): T =>
	useStore(useRun(source, run).store, pick);

/** The line above the rows. */
export const useRunHeader = (source: DataSource, run: RunKey): RunHeader | null =>
	useStore(useRun(source, run).store, (state) => state.header);

/** The run's loops, for the loop gutter. */
export const useRunLoops = (source: DataSource, run: RunKey): readonly LoopState[] =>
	useStore(useRun(source, run).store, (state) => state.loops);

/** Whether the run loaded, and its notice or error. */
export type RunLoad = Pick<RunState, "status" | "notice" | "error">;

/** Whether the run loaded, and what's wrong with it if anything. */
export const useRunLoad = (source: DataSource, run: RunKey): RunLoad => {
	const store = useRun(source, run).store;
	const status = useStore(store, (state) => state.status);
	const notice = useStore(store, (state) => state.notice);
	const error = useStore(store, (state) => state.error);
	return { status, notice, error };
};
