// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What every row of one agent's feed shares. The value changes only when one
// of these does, so a reply streaming in redraws its own row and nothing else.
// A row reads its item from the store.

import { createContext, type ReactNode, useContext, useMemo } from "react";
import { useStore } from "zustand";

import type { Agent, AgentId, TimelineItem, Vendor } from "@maxhq/shared/bindings";
import type { TimelineState } from "@maxhq/shared/timeline/apply";
import { type FeedRow, feedRows } from "@maxhq/shared/timeline/output";
import type { TimelineStore } from "@maxhq/shared/timeline/store";

type Feed = {
	readonly agent: AgentId;
	readonly vendor: Vendor;
	/** The device the agent runs on, by name. */
	readonly device: string;
	readonly store: TimelineStore;
	/**
	 * Set for a session in a terminal, which MaxHQ watches but can't answer,
	 * or a sub-agent that finished.
	 */
	readonly readOnly: boolean;
};

const Context = createContext<Feed | null>(null);

/** Hands a feed's shared values to its rows. */
export const FeedProvider = ({
	agent,
	device,
	store,
	readOnly = false,
	children,
}: {
	agent: Pick<Agent, "id" | "vendor" | "control">;
	device: string;
	store: TimelineStore;
	/** Set for a feed nothing answers, such as a finished sub-agent's. */
	readOnly?: boolean;
	children: ReactNode;
}) => {
	const { id, vendor, control } = agent;
	const shut = readOnly || control === "observed";
	const value = useMemo(
		() => ({ agent: id, vendor, device, store, readOnly: shut }),
		[id, vendor, device, store, shut]
	);
	return <Context value={value}>{children}</Context>;
};

/** The shared values from the nearest `FeedProvider`. */
export const useFeed = (): Feed => {
	const value = useContext(Context);
	if (!value) throw new Error("useFeed needs a FeedProvider above it");
	return value;
};

// Reads the rows from the state, working them out again only when the
// timeline's list of rows has changed.
const rowsSelector = (keep?: (item: TimelineItem) => boolean) => {
	let last: TimelineState["rows"] | null = null;
	let rows: FeedRow[] = [];
	return (state: TimelineState) => {
		if (state.rows !== last) {
			last = state.rows;
			rows = feedRows(state, keep);
		}
		return rows;
	};
};

/**
 * The feed's rows, with only the items `keep` keeps. They change only when
 * an item comes or goes, not as text streams in. Pass a `keep` that stays the
 * same between renders.
 */
export const useFeedRows = (
	store: TimelineStore,
	keep?: (item: TimelineItem) => boolean
): FeedRow[] => {
	const select = useMemo(() => rowsSelector(keep), [keep]);
	return useStore(store, select);
};
