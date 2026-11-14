// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Going to the request an agent waits on: the approval or question card in
// its timeline. The composer's "Go to the request", ⌘J and the bell ask for
// it. The agent's list scrolls to the card and focuses it, so Enter answers
// an approval and 1 to 9 pick a question's options. A list that isn't
// mounted yet takes the ask when it mounts, if that's soon.

import type { AgentId, AttentionId, ItemId, TimelineItem } from "@maxhq/shared/bindings";
import type { TimelineState } from "@maxhq/shared/timeline/apply";

import { isOpen } from "@maxhq/shared/agent-view/format";
import { bringIntoView, type JumpTarget, seekItem } from "./jump";

/** Marks the card that waits on you, which a request focuses. `WaitingCard` sets it. */
export const WAITING_CARD = "data-waiting-card";

/** How long an ask waits for the list to mount, then for its card to come. */
export const ASK_MS = 5_000;

/** One ask to go to an agent's request. */
export type RequestAsk = {
	/** The request to go to, when the agent's activity names one. */
	readonly attention: AttentionId | null;
	readonly at: number;
};

const asks = new Map<AgentId, RequestAsk>();
const listeners = new Map<AgentId, Set<() => void>>();

/** Asks the agent's list to go to the request it waits on, and tells it. */
export const askForRequest = (agent: AgentId, attention: AttentionId | null): void => {
	asks.set(agent, { attention, at: Date.now() });
	for (const listener of [...(listeners.get(agent) ?? [])]) listener();
};

/** Takes the agent's ask, unless there's none or it's gone stale. */
export const takeRequest = (agent: AgentId): RequestAsk | null => {
	const ask = asks.get(agent);
	asks.delete(agent);
	return ask && Date.now() - ask.at <= ASK_MS ? ask : null;
};

/** Hears each ask for the agent. Returns the undo. */
export const watchRequests = (agent: AgentId, listener: () => void): (() => void) => {
	const set = listeners.get(agent) ?? new Set();
	set.add(listener);
	listeners.set(agent, set);
	return () => {
		set.delete(listener);
		if (set.size === 0 && listeners.get(agent) === set) listeners.delete(agent);
	};
};

/** Forgets every ask, for tests. */
export const clearRequests = () => {
	asks.clear();
	listeners.clear();
};

type Request = Extract<TimelineItem, { kind: "approval" | "question" }>;

const waits = (item: TimelineItem): item is Request =>
	isOpen(item.status) &&
	((item.kind === "approval" && item.decided === null) ||
		(item.kind === "question" && item.answers === null));

/**
 * The card waiting on you: the one for `attention`, or else the newest that
 * waits. Null when none does.
 */
export const waitingItem = (
	state: Pick<TimelineState, "items" | "rows">,
	attention: AttentionId | null
): ItemId | null => {
	let newest: ItemId | null = null;
	for (const id of state.rows) {
		const item = state.items.get(id);
		if (!item || !waits(item)) continue;
		if (attention !== null && item.attention === attention) return id;
		newest = id;
	}
	return newest;
};

/**
 * Scrolls the list to the card waiting on you and focuses it, once the
 * timeline has it. A card that already shows stays where it is. Gives up
 * after `ASK_MS`. Returns the call that stops it.
 */
export const goToRequest = (target: JumpTarget, attention: AttentionId | null): (() => void) => {
	const { timeline, scroller } = target;
	let stopSeek: (() => void) | null = null;
	let done = false;
	const stop = () => {
		done = true;
		unsubscribe();
		clearTimeout(timer);
		stopSeek?.();
	};
	const shown = (row: HTMLElement) => {
		stop();
		const card = row.querySelector<HTMLElement>(`[${WAITING_CARD}]`);
		if (!card) return;
		bringIntoView(scroller, card.getBoundingClientRect());
		// After a shortcut that opened the pane hands the focus to its composer.
		setTimeout(() => card.focus({ preventScroll: true }), 0);
	};
	const check = () => {
		if (done || stopSeek) return;
		const state = timeline.store.getState();
		if (state.status === "failed") return stop();
		if (state.status !== "ready") return;
		const item = waitingItem(state, attention);
		if (item === null) return;
		unsubscribe();
		stopSeek = seekItem(target, item, shown, stop, { center: false });
	};
	const unsubscribe = timeline.store.subscribe(check);
	const timer = setTimeout(stop, ASK_MS);
	check();
	return stop;
};
