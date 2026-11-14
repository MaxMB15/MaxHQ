// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where a visual came from: the agent's reply that holds it. Tell the agent
// quotes a visual's errors from that reply into that agent's composer, so a
// visual elsewhere, such as in a file, has no one to tell.

import { createContext, use } from "react";

import type { AgentId, TimelineItem } from "@maxhq/shared/bindings";

export type VisualOrigin = {
	readonly agent: { readonly id: AgentId; readonly title: string };
	readonly item: TimelineItem;
};

export const VisualOriginContext = createContext<VisualOrigin | null>(null);

/** The reply around a visual, when it's in one whose agent takes messages. */
export const useVisualOrigin = (): VisualOrigin | null => use(VisualOriginContext);
