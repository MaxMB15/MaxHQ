// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { useServices } from "@/app/services";
import type { AgentId } from "@maxhq/shared/bindings";
import type { Timeline } from "@maxhq/shared/timeline/store";
import { useTimeline as useTimelineOf } from "@maxhq/shared/timeline/use-timeline";

/** The agent's timeline, open for as long as the calling view is mounted. */
export const useTimeline = (agent: AgentId): Timeline => useTimelineOf(useServices().source, agent);
