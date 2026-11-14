// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The workflow run hooks, reading from this window's source. Each holds the
// run open while the calling view is mounted.

import { useServices } from "@/app/services";
import type { LoopState, NodeState, RunHeader, RunKey, RunShape } from "@maxhq/shared/bindings";
import type { RunState } from "@maxhq/shared/workflow/store";
import * as shared from "@maxhq/shared/workflow/use-run";

/** The run's shape, which keeps its identity until its hash changes. */
export const useRunShape = (run: RunKey): RunShape | null =>
	shared.useRunShape(useServices().source, run);

/** One node's state. */
export const useNode = (run: RunKey, id: string): NodeState | undefined =>
	shared.useNode(useServices().source, run, id);

/** Every node's state, each keeping its object until it changes. */
export const useRunNodes = (run: RunKey): ReadonlyMap<string, NodeState> =>
	shared.useRunNodes(useServices().source, run);

/** One value worked out from the run, which renders the view again only when it changes. */
export const useRunValue = <T>(run: RunKey, pick: (state: RunState) => T): T =>
	shared.useRunValue(useServices().source, run, pick);

/** The line above the rows. */
export const useRunHeader = (run: RunKey): RunHeader | null =>
	shared.useRunHeader(useServices().source, run);

/** The run's loops, for the loop gutter. */
export const useRunLoops = (run: RunKey): readonly LoopState[] =>
	shared.useRunLoops(useServices().source, run);

/** Whether the run loaded, and what's wrong with it if anything. */
export const useRunLoad = (run: RunKey): shared.RunLoad =>
	shared.useRunLoad(useServices().source, run);
