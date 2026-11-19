// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The origin a feed row hands the visuals in it, so Tell the agent puts
// their errors in that agent's box. Only a row of the agent's own feed has
// one, and only when a quote can go there now: the feed answers, the box
// shows and the reply has finished. Anywhere else, such as a note or a
// visual shown whole, a card has no origin and so nothing to fill.

import { useMemo } from "react";

import type { TimelineItem } from "@maxhq/shared/bindings";
import type { VisualOrigin } from "@maxhq/shared/visuals/origin";

import { useView } from "../../services";
import { useFeed } from "../agent/feed-context";
import { useBoxShown } from "../composer/open-boxes";

/** Where the row's quotes and visuals go, or null while nothing can take them. */
export const useFeedOrigin = (item: TimelineItem): VisualOrigin | null => {
	const { agent, readOnly } = useFeed();
	const title = useView((state) => state.view.tables.agent.get(agent)?.title ?? "");
	const shown = useBoxShown(agent);
	const open = !readOnly && shown && !(item.kind === "assistant_message" && item.streaming);
	return useMemo(
		() => (open ? { agent: { id: agent, title }, item } : null),
		[open, agent, title, item]
	);
};
