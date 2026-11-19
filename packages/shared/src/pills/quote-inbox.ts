// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Quotes, command output and visuals on their way to an agent's composer,
// from Add to chat, the timeline's menu, a command you ran with `!` or a
// drop on the agent in the sidebar. A draft
// lives in its composer, so each waits here until that composer takes it:
// right away when it's mounted, or when it mounts.

import type { AgentId } from "../bindings";
import type { OutputRef, QuoteRef, SessionRef } from "./draft";
import type { VisualTransfer } from "../visuals/transfer";

/** One quote, output or visual for a composer. */
export type InboundQuote = (
	| { readonly quote: QuoteRef | OutputRef | SessionRef; readonly visual?: undefined }
	| { readonly visual: VisualTransfer; readonly quote?: undefined }
) & {
	/** Whether the composer takes the focus as it adds the pill. */
	readonly focus: boolean;
};

/** The quotes and visuals waiting for each agent's composer. */
export type QuoteInbox = {
	/** Queues a quote, a command's output or a session for an agent's composer, and tells it. */
	readonly send: (
		agent: AgentId,
		quote: QuoteRef | OutputRef | SessionRef,
		focus: boolean
	) => void;
	/** Queues a visual for an agent's composer, and tells it. */
	readonly sendVisual: (agent: AgentId, visual: VisualTransfer, focus: boolean) => void;
	/** Takes the quotes waiting for an agent, oldest first. */
	readonly take: (agent: AgentId) => InboundQuote[];
	/** Hears each quote sent to an agent. Returns the undo. */
	readonly watch: (agent: AgentId, listener: () => void) => () => void;
};

export const createQuoteInbox = (): QuoteInbox => {
	const waiting = new Map<AgentId, InboundQuote[]>();
	const listeners = new Map<AgentId, Set<() => void>>();

	const queue = (agent: AgentId, inbound: InboundQuote) => {
		const waits = waiting.get(agent) ?? [];
		waits.push(inbound);
		waiting.set(agent, waits);
		for (const listener of [...(listeners.get(agent) ?? [])]) listener();
	};

	return {
		send: (agent, quote, focus) => queue(agent, { quote, focus }),
		sendVisual: (agent, visual, focus) => queue(agent, { visual, focus }),
		take: (agent) => {
			const queue = waiting.get(agent) ?? [];
			waiting.delete(agent);
			return queue;
		},
		watch: (agent, listener) => {
			const set = listeners.get(agent) ?? new Set();
			set.add(listener);
			listeners.set(agent, set);
			return () => {
				set.delete(listener);
				if (set.size === 0 && listeners.get(agent) === set) listeners.delete(agent);
			};
		},
	};
};

/** The window's inbox, which the timeline, the sidebar and every composer share. */
export const quoteInbox: QuoteInbox = createQuoteInbox();
