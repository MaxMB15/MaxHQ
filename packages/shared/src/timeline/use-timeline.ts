// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The open timeline for a view, held while the view is mounted. Views of
// the same agent share one.

import type { AgentId } from "../bindings";
import type { DataSource } from "../data/source";
import { useSharedOpen } from "../data/use-shared-open";
import { timelineOpens } from "./registry";
import type { Timeline } from "./store";

/** The agent's timeline, open for as long as the calling view is mounted. */
export const useTimeline = (source: DataSource, agent: AgentId): Timeline =>
	useSharedOpen(timelineOpens, source, agent);
