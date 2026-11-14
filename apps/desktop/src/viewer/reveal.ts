// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The line a file pane was last asked to show. Opening a file at a line
// asks here, apart from the pane's own params, so one file keeps one pane
// and a pane that's already open still hears the ask. Each ask has its own
// number, so asking for the same line again scrolls to it again.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { AgentId } from "@maxhq/shared/bindings";

/** A line to show, and which ask it came from. */
export type Reveal = { readonly line: number; readonly ask: number };

const store = createStore<{ asked: ReadonlyMap<string, Reveal> }>(() => ({
	asked: new Map(),
}));

const keyFor = (agent: AgentId, path: string) => `${agent}\n${path}`;

let asks = 0;

/**
 * Asks the pane for `agent`'s file at `path` to scroll to `line` and mark
 * it. Without a line, it forgets the last ask, so a file opened again
 * without one doesn't jump back to an old line.
 */
export const revealLine = (agent: AgentId, path: string, line?: number): void => {
	const key = keyFor(agent, path);
	if (line === undefined || !Number.isInteger(line) || line < 1) {
		if (!store.getState().asked.has(key)) return;
		store.setState(({ asked }) => {
			const next = new Map(asked);
			next.delete(key);
			return { asked: next };
		});
		return;
	}
	asks += 1;
	const reveal: Reveal = { line, ask: asks };
	store.setState(({ asked }) => ({ asked: new Map(asked).set(key, reveal) }));
};

/** The line the pane for `agent`'s file at `path` was last asked to show, if any. */
export const useReveal = (agent: AgentId, path: string): Reveal | null =>
	useStore(store, (s) => s.asked.get(keyFor(agent, path)) ?? null);

/** Forgets every ask, for tests. */
export const clearReveals = (): void => store.setState({ asked: new Map() });

/** The line the pane for `agent`'s file at `path` was last asked to show, outside React. */
export const revealed = (agent: AgentId, path: string): Reveal | null =>
	store.getState().asked.get(keyFor(agent, path)) ?? null;
