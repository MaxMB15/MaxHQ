// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What every row of one agent view shares: whose timeline it is, whether
// the session is read only or on another device, and the calls a card makes. The value changes
// only when one of those does, so a reply streaming in re-renders its own
// row and nothing else. A row that needs another item, such as the command
// an approval is for, reads it from the store.

import { createContext, type ReactNode, useContext } from "react";

import type { ActionResult } from "../data/source";
import type { AgentId, AttentionAnswer, AttentionId, Vendor } from "../bindings";
import type { InlineRef } from "../pills/wire";
import type { TimelineStore } from "../timeline/store";

/** What the rows of one agent view share. */
export type RowContext = {
	readonly agent: AgentId;
	readonly vendor: Vendor;
	/** The agent's timeline, for rows that look at other items or the queue. */
	readonly timeline: TimelineStore;
	/** Set for a session in a terminal, which MaxHQ watches but can't answer. */
	readonly readOnly: boolean;
	/** Set when the agent runs on another device, so its files download to this Mac. */
	readonly remote: boolean;
	/** Sends your answer to an approval or a question, with the pills you typed in it. */
	readonly answer: (
		attention: AttentionId,
		answer: AttentionAnswer,
		refs?: readonly InlineRef[]
	) => Promise<ActionResult>;
	/** Whether this agent's sub-agents show in the side panel. */
	readonly subAgentsShown: boolean;
	/** The sub-agent whose row the side panel points out, if any. */
	readonly shownSubAgent?: AgentId | null;
	/**
	 * Opens the side panel on this agent's sub-agents and points out one
	 * sub-agent's row, or closes the panel when it already points that row out.
	 */
	readonly toggleSubAgents: (subAgent: AgentId) => void;
};

const Context = createContext<RowContext | null>(null);

/** Hands a view's shared values to its rows. */
export const RowProvider = ({ value, children }: { value: RowContext; children: ReactNode }) => (
	<Context value={value}>{children}</Context>
);

/** The shared values from the nearest `RowProvider`. */
export const useRow = (): RowContext => {
	const value = useContext(Context);
	if (!value) throw new Error("useRow needs a RowProvider above it");
	return value;
};
