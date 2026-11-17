// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which commands' output waits in each agent's composer, so a "You ran"
// card can say its output goes with the next message, and Keep private
// can take the pill back out.

import { type RefObject, useCallback, useEffect, useSyncExternalStore } from "react";

import type { AgentId, TerminalId } from "@maxhq/shared/bindings";
import type { Segment } from "@maxhq/shared/pills/draft";

import type { PromptEditorHandle } from "./editor";

type Held = {
	readonly terminals: ReadonlySet<TerminalId>;
	readonly remove: (terminal: TerminalId) => void;
};

const held = new Map<AgentId, Held>();
const listeners = new Set<() => void>();

const changed = () => {
	for (const listener of [...listeners]) listener();
};

const subscribe = (listener: () => void) => {
	listeners.add(listener);
	return () => listeners.delete(listener);
};

/** The terminals whose output pills are in these segments. */
export const outputTerminals = (segments: readonly Segment[]): Set<TerminalId> => {
	const terminals = new Set<TerminalId>();
	for (const segment of segments)
		if (segment.type === "pill" && segment.pill.type === "output" && segment.pill.terminal)
			terminals.add(segment.pill.terminal);
	return terminals;
};

/** Where in the text the output pill of this terminal sits, or null when it's not there. */
export const outputOffset = (segments: readonly Segment[], terminal: TerminalId): number | null => {
	let offset = 0;
	for (const segment of segments) {
		if (segment.type === "text") {
			offset += segment.text.length;
			continue;
		}
		if (segment.pill.type === "output" && segment.pill.terminal === terminal) return offset;
		offset += 1;
	}
	return null;
};

const sameSet = (a: ReadonlySet<TerminalId>, b: ReadonlySet<TerminalId>) =>
	a.size === b.size && [...a].every((terminal) => b.has(terminal));

/**
 * Tells the cards which output the composer holds, while it's mounted,
 * and lets Keep private take a pill back out of its text box.
 */
export const useHeldOutputs = (
	agent: AgentId,
	segments: readonly Segment[],
	editor: RefObject<PromptEditorHandle | null>
) => {
	const remove = useCallback(
		(terminal: TerminalId) => {
			const handle = editor.current;
			const at = handle ? outputOffset(handle.segments(), terminal) : null;
			if (handle && at !== null) handle.replaceRange(at, at + 1, []);
		},
		[editor]
	);
	useEffect(() => {
		const terminals = outputTerminals(segments);
		const before = held.get(agent);
		if (before && sameSet(before.terminals, terminals) && before.remove === remove) return;
		if (terminals.size === 0) held.delete(agent);
		else held.set(agent, { terminals, remove });
		if (before || terminals.size > 0) changed();
	}, [agent, segments, remove]);
	useEffect(
		() => () => {
			if (held.delete(agent)) changed();
		},
		[agent]
	);
};

/** Whether this command's output waits in its agent's composer. */
export const useOutputHeld = (agent: AgentId, terminal: TerminalId): boolean =>
	useSyncExternalStore(subscribe, () => held.get(agent)?.terminals.has(terminal) ?? false);

/** Takes this command's output out of its agent's composer, for Keep private or once it went. */
export const takeOutputBack = (agent: AgentId, terminal: TerminalId): void =>
	held.get(agent)?.remove(terminal);
