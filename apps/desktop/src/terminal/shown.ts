// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether a terminal shows in this window's terminal panel: as a tab its
// agent's open drawer shows, alone or beside another, or as a pane of its
// own. A card that sends you to the terminal offers Open in terminal only
// while it doesn't.

import { useEffect, useState } from "react";

import { type Services, useServices, useView } from "@/app/services";
import { panesOf } from "@/layout/panes";
import type { AgentId, TerminalId } from "@maxhq/shared/bindings";
import { terminalsOf } from "@maxhq/shared/terminal/select";

import { activeOf, poppedOut } from "./actions";
import { drawerOf, useDrawers } from "./drawer-prefs";

const same = (a: ReadonlySet<TerminalId>, b: ReadonlySet<TerminalId>) =>
	a.size === b.size && [...a].every((id) => b.has(id));

/** The terminals open as panes of their own, kept up to date. */
export const usePopped = (services: Services): ReadonlySet<TerminalId> => {
	const [popped, setPopped] = useState(() => poppedOut(services));
	useEffect(() => {
		let stops: { dispose: () => void }[] = [];
		const unwatch = panesOf(services).watch((api) => {
			for (const stop of stops) stop.dispose();
			const update = () =>
				setPopped((before) => {
					const now = poppedOut(services);
					return same(before, now) ? before : now;
				});
			stops = api ? [api.onDidAddPanel(update), api.onDidRemovePanel(update)] : [];
			update();
		});
		return () => {
			unwatch();
			for (const stop of stops) stop.dispose();
		};
	}, [services]);
	return popped;
};

/**
 * Which of the agent's terminals its open drawer shows, given the ones out
 * in panes: the active tab and the one beside it. Empty while it's closed.
 */
export const drawerShows = (
	open: boolean,
	tabs: readonly { readonly id: TerminalId }[],
	picked: TerminalId | undefined,
	beside: TerminalId | undefined
): TerminalId[] => {
	if (!open) return [];
	const active = activeOf(tabs, picked);
	if (!active) return [];
	return beside !== undefined && beside !== active.id && tabs.some((tab) => tab.id === beside)
		? [active.id, beside]
		: [active.id];
};

/** Whether this window shows the terminal in the agent's drawer or in a pane of its own. */
export const useShownInPanel = (agent: AgentId, terminal: TerminalId): boolean => {
	const services = useServices();
	const popped = usePopped(services);
	const open = useDrawers((s) => drawerOf(s, agent).open);
	const picked = useDrawers((s) => s.active[agent]);
	const beside = useDrawers((s) => s.split[agent]);
	const ids = useView((s) =>
		terminalsOf(s.view, agent)
			.map((tab) => tab.id)
			.join("\n")
	);
	if (popped.has(terminal)) return true;
	const tabs = ids === "" ? [] : ids.split("\n").filter((id) => !popped.has(id));
	return drawerShows(
		open,
		tabs.map((id) => ({ id })),
		picked,
		beside
	).includes(terminal);
};
