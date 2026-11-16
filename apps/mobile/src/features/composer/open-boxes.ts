// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agents whose message box the phone shows now. A reply offers Quote
// only where a box takes the quote, so a quote never waits unseen for a box
// the phone doesn't show, such as the dispatcher's.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { AgentId } from "@maxhq/shared/bindings";

type OpenBoxes = { readonly open: ReadonlyMap<AgentId, number> };

/** How many boxes each agent has open. Exported for tests. */
export const openBoxes = createStore<OpenBoxes>(() => ({ open: new Map() }));

const count = (agent: AgentId, by: number) =>
	openBoxes.setState(({ open }) => {
		const next = new Map(open);
		const now = (next.get(agent) ?? 0) + by;
		if (now > 0) next.set(agent, now);
		else next.delete(agent);
		return { open: next };
	});

/** Marks an agent's box as shown. Returns what marks it shut again. */
export const openBox = (agent: AgentId): (() => void) => {
	count(agent, 1);
	let shut = false;
	return () => {
		if (shut) return;
		shut = true;
		count(agent, -1);
	};
};

/** Whether the phone shows a box for the agent now. */
export const boxShown = (agent: AgentId): boolean => openBoxes.getState().open.has(agent);

/** Whether the phone shows a box for the agent, as it changes. */
export const useBoxShown = (agent: AgentId): boolean =>
	useStore(openBoxes, ({ open }) => open.has(agent));
