// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A file the agent opens for you shows in a new tab beside the agent's pane,
// but only when you're watching. The item must arrive live while this
// window shows the agent, the agent's pane must be the tab its group shows,
// and the window must have focus. A first load, an older page, a reconnect
// or a replay never opens one, and each item opens once at most. As a last
// guard, an item that began more than a couple of minutes ago never opens,
// in case a catch-up crossed a live change on the way and passed for one.

import { useEffect } from "react";

import { useServices, type Services } from "@/app/services";
import { paneId } from "@/layout/pane-id";
import { panesOf } from "@/layout/panes";
import { windowState } from "@/layout/shown";
import type { AgentId, ItemId, TimelineItem } from "@maxhq/shared/bindings";
import type { Timeline } from "@maxhq/shared/timeline/store";

// The items this window opened, so one never opens twice.
const opened = new Set<ItemId>();

// How long ago an item may have begun and still open. Its time comes from
// the agent's device, so this leaves room for a clock that's a little off.
const FRESH_MS = 2 * 60 * 1000;

// Whether an item began recently, or didn't say when.
const fresh = (item: TimelineItem, now: number): boolean =>
	item.started_at === null || now - item.started_at <= FRESH_MS;

/** Forgets what opened, for tests. */
export const forgetOpened = (): void => opened.clear();

/**
 * Opens the files among `items` that the agent opened for you, as a tab in
 * the group of the agent's pane, when that pane shows in a window with focus.
 * `items` are the ones that arrived live.
 */
export const openArrived = (
	services: Services,
	agent: AgentId,
	items: readonly TimelineItem[],
	focused: () => boolean = () => windowState().focused,
	now: number = Date.now()
): void => {
	const files = items.filter(
		(item) => item.kind === "agent_opened_file" && !opened.has(item.id) && fresh(item, now)
	);
	if (files.length === 0) return;
	// The panes stay mounted under the devices view, but you can't see them.
	if (services.ui.getState().mode !== "projects" || !focused()) return;
	const panes = panesOf(services);
	const panel = panes.api()?.getPanel(paneId({ kind: "agent", agent }));
	if (!panel) return;
	const { group } = panel;
	if (group.activePanel !== panel || !group.api.isVisible) return;
	if (group.api.location.type === "popout") return;
	for (const item of files) {
		if (item.kind !== "agent_opened_file") continue;
		opened.add(item.id);
		panes.openFile(agent, item.path, {
			at: { group: group.id, side: "center" },
			...(item.line === null ? {} : { line: item.line }),
		});
	}
};

/** Opens the files the agent opens for you while its timeline is open here. */
export const useAutoOpen = (agent: AgentId, timeline: Timeline): void => {
	const services = useServices();
	useEffect(
		() => timeline.onArrive((items) => openArrived(services, agent, items)),
		[services, agent, timeline]
	);
};
