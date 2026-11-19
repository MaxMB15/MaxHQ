// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which pills wait in each agent's composer, so a card can say its output
// goes with the next message and take its pill back out. The output pills
// of "You ran" cards go by terminal, and session pills by card.

import { type RefObject, useCallback, useEffect, useSyncExternalStore } from "react";

import type { AgentId, ItemId, TerminalId } from "@maxhq/shared/bindings";
import type { PlacedPill, Segment } from "@maxhq/shared/pills/draft";

import type { PromptEditorHandle } from "./editor";

/** Where in the text the first pill that matches sits, or null when none is there. */
export const pillOffset = (
	segments: readonly Segment[],
	matches: (pill: PlacedPill) => boolean
): number | null => {
	let offset = 0;
	for (const segment of segments) {
		if (segment.type === "text") {
			offset += segment.text.length;
			continue;
		}
		if (matches(segment.pill)) return offset;
		offset += 1;
	}
	return null;
};

/** Takes the first pill that matches out of the text box. */
export const removePill = (
	handle: PromptEditorHandle | null,
	matches: (pill: PlacedPill) => boolean
): void => {
	const at = handle ? pillOffset(handle.segments(), matches) : null;
	if (handle && at !== null) handle.replaceRange(at, at + 1, []);
};

const sameSet = <K>(a: ReadonlySet<K>, b: ReadonlySet<K>) =>
	a.size === b.size && [...a].every((key) => b.has(key));

/** The pills of one kind that each agent's composer holds, by the key `keyOf` gives. */
const heldPills = <K extends string>(keyOf: (pill: PlacedPill) => K | null) => {
	type Held = { readonly keys: ReadonlySet<K>; readonly remove: (key: K) => void };
	const held = new Map<AgentId, Held>();
	const listeners = new Set<() => void>();

	const changed = () => {
		for (const listener of [...listeners]) listener();
	};

	const subscribe = (listener: () => void) => {
		listeners.add(listener);
		return () => listeners.delete(listener);
	};

	/** The keys of the pills in these segments. */
	const keysIn = (segments: readonly Segment[]): Set<K> => {
		const keys = new Set<K>();
		for (const segment of segments) {
			const key = segment.type === "pill" ? keyOf(segment.pill) : null;
			if (key) keys.add(key);
		}
		return keys;
	};

	/**
	 * Tells the cards which pills the composer holds, while it's mounted,
	 * and lets them take a pill back out of its text box.
	 */
	const useHeld = (
		agent: AgentId,
		segments: readonly Segment[],
		editor: RefObject<PromptEditorHandle | null>
	) => {
		const remove = useCallback(
			(key: K) => removePill(editor.current, (pill) => keyOf(pill) === key),
			[editor]
		);
		useEffect(() => {
			const keys = keysIn(segments);
			const before = held.get(agent);
			if (before && sameSet(before.keys, keys) && before.remove === remove) return;
			if (keys.size === 0) held.delete(agent);
			else held.set(agent, { keys, remove });
			if (before || keys.size > 0) changed();
		}, [agent, segments, remove]);
		useEffect(
			() => () => {
				if (held.delete(agent)) changed();
			},
			[agent]
		);
	};

	/** Whether the agent's composer holds this pill. */
	const useIsHeld = (agent: AgentId, key: K): boolean =>
		useSyncExternalStore(subscribe, () => held.get(agent)?.keys.has(key) ?? false);

	/** Takes this pill out of the agent's composer. */
	const take = (agent: AgentId, key: K): void => held.get(agent)?.remove(key);

	return { keysIn, useHeld, useIsHeld, take };
};

const outputs = heldPills<TerminalId>((pill) =>
	pill.type === "output" ? (pill.terminal ?? null) : null
);

/** The session pill of this card. */
export const isSessionOf = (item: ItemId) => (pill: PlacedPill) =>
	pill.type === "session" && pill.item === item;

const sessions = heldPills<ItemId>((pill) =>
	pill.type === "session" ? (pill.item ?? null) : null
);

/** Tells the "You ran" cards which output the composer holds. */
export const useHeldOutputs = outputs.useHeld;

/** Whether this command's output waits in its agent's composer. */
export const useOutputHeld = outputs.useIsHeld;

/** Takes this command's output out of its agent's composer, for Keep private or once it went. */
export const takeOutputBack = outputs.take;

/** The cards whose session pills are in these segments. */
export const sessionItems = sessions.keysIn;

/** Tells the session cards whose pill the composer holds. */
export const useHeldSessions = sessions.useHeld;

/** Whether the agent's composer holds this card's pill. */
export const useSessionHeld = sessions.useIsHeld;

/** Takes this card's pill out of its agent's composer, once it went by itself. */
export const takeSessionBack = sessions.take;
