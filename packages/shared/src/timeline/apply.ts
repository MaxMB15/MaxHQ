// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One agent's timeline as the webview holds it, and the pure function that
// applies each update from `maxd`. Items live in a map by ID, beside a list
// of IDs in ordinal order. A row reads its own item from the map, so a
// streaming reply touches one entry and leaves the lists alone. Each batch
// makes a new version of the map that shares the old one's entries, so a
// long timeline isn't copied every frame.

import type {
	ItemId,
	Seq,
	TextField,
	TimelineEvent,
	TimelineItem,
	TimelineUpdate,
} from "../bindings";
import { draft } from "../lib/layered-map";

/** Whether the first page has come in. */
export type TimelineStatus = "loading" | "ready" | "failed";

/** One agent's timeline. */
export type TimelineState = {
	readonly status: TimelineStatus;
	/** Why it failed to open, or why older items failed to load. */
	readonly error: string | null;
	/** The version of the last update applied. */
	readonly seq: Seq;
	/** Every item the webview has, by ID. */
	readonly items: ReadonlyMap<ItemId, TimelineItem>;
	/** Every item's ID, by ordinal. Replaced only when an item comes or goes. */
	readonly order: readonly ItemId[];
	/** The IDs the timeline shows, by ordinal. Queued and cancelled messages are left out. */
	readonly rows: readonly ItemId[];
	/** Messages waiting for the turn to end, oldest first, for the composer. */
	readonly queued: readonly TimelineItem[];
	/** Whether `maxd` has items before the first one here. */
	readonly hasOlder: boolean;
	/** Set while a page of older items is on its way. */
	readonly loadingOlder: boolean;
};

/** A timeline before its first page. */
export const EMPTY_TIMELINE: TimelineState = {
	status: "loading",
	error: null,
	seq: 0,
	items: new Map(),
	order: [],
	rows: [],
	queued: [],
	hasOlder: false,
	loadingOlder: false,
};

const isHidden = (item: TimelineItem) =>
	item.kind === "user_message" && (item.delivery === "queued" || item.delivery === "cancelled");

const isQueued = (item: TimelineItem) => item.kind === "user_message" && item.delivery === "queued";

// Rows and the queue, worked out again after the list or a message's delivery changes.
const derive = (items: ReadonlyMap<ItemId, TimelineItem>, order: readonly ItemId[]) => {
	const rows: ItemId[] = [];
	const queued: TimelineItem[] = [];
	for (const id of order) {
		const item = items.get(id);
		if (!item) continue;
		if (isQueued(item)) queued.push(item);
		if (!isHidden(item)) rows.push(id);
	}
	return { rows, queued };
};

const byOrdinal = (items: ReadonlyMap<ItemId, TimelineItem>) =>
	[...items.values()].sort((a, b) => a.ordinal - b.ordinal).map((item) => item.id);

// Where an item with this ordinal goes. Most land at the end, so check there first.
const insertAt = (
	items: Pick<ReadonlyMap<ItemId, TimelineItem>, "get">,
	order: readonly ItemId[],
	ordinal: number
): number => {
	const last = order.length > 0 ? items.get(order[order.length - 1] as ItemId) : undefined;
	if (!last || last.ordinal < ordinal) return order.length;
	let low = 0;
	let high = order.length;
	while (low < high) {
		const mid = (low + high) >> 1;
		const at = items.get(order[mid] as ItemId);
		if (at && at.ordinal < ordinal) low = mid + 1;
		else high = mid;
	}
	return low;
};

/** Adds streamed text to the field it names, if the item has that field. */
export const withText = (item: TimelineItem, field: TextField, text: string): TimelineItem => {
	if (field === "output") {
		if (item.kind === "command") return { ...item, output: item.output + text };
		if (item.kind === "tool_call") return { ...item, output: (item.output ?? "") + text };
		return item;
	}
	switch (item.kind) {
		case "assistant_message":
		case "reasoning":
		case "user_message":
			return { ...item, text: item.text + text };
		case "plan":
			return { ...item, markdown: item.markdown + text };
		default:
			return item;
	}
};

const applyEvents = (state: TimelineState, seq: Seq, events: readonly TimelineEvent[]) => {
	const items = draft(state.items);
	let order: ItemId[] | null = null;
	let reshape = false;
	// Appends to one item within a batch join once, at the end or before an upsert replaces it.
	const chunks = new Map<ItemId, { field: TextField; parts: string[] }[]>();
	// Removes leave the list in one pass, at the end or before an upsert moves an item, so
	// a burst of them, such as the oldest items past a device's cap, costs one copy.
	let gone: Set<ItemId> | null = null;

	const dropGone = () => {
		if (!gone) return;
		const leaving = gone;
		gone = null;
		order = (order ?? state.order).filter((id) => !leaving.has(id));
	};

	const flushChunks = (id: ItemId) => {
		const runs = chunks.get(id);
		if (!runs) return;
		chunks.delete(id);
		let item = items.get(id);
		if (!item) return;
		for (const run of runs) item = withText(item, run.field, run.parts.join(""));
		items.set(id, item);
	};

	for (const event of events) {
		if (event.type === "append") {
			if (!items.has(event.id)) continue;
			const runs = chunks.get(event.id) ?? [];
			const last = runs[runs.length - 1];
			if (last && last.field === event.field) last.parts.push(event.text);
			else runs.push({ field: event.field, parts: [event.text] });
			chunks.set(event.id, runs);
			continue;
		}
		if (event.type === "remove") {
			chunks.delete(event.id);
			if (!items.delete(event.id)) continue;
			(gone ??= new Set()).add(event.id);
			reshape = true;
			continue;
		}
		const item = event.item;
		// The upsert carries the whole item, text included, so earlier appends are moot.
		chunks.delete(item.id);
		const before = items.get(item.id);
		if (before && before.ordinal === item.ordinal) {
			items.set(item.id, item);
			if (isHidden(before) !== isHidden(item) || isQueued(before) || isQueued(item)) {
				reshape = true;
			}
			continue;
		}
		dropGone();
		order ??= [...state.order];
		if (before) order.splice(order.indexOf(item.id), 1);
		items.set(item.id, item);
		order.splice(insertAt(items, order, item.ordinal), 0, item.id);
		reshape = true;
	}
	for (const id of [...chunks.keys()]) flushChunks(id);
	dropGone();

	const next = items.done();
	const nextOrder = order ?? state.order;
	const derived = reshape ? derive(next, nextOrder) : { rows: state.rows, queued: state.queued };
	return { ...state, seq, items: next, order: nextOrder, ...derived };
};

/**
 * Applies one update. A reset replaces everything, events apply in order
 * unless they're older than what's here, and an older page goes in front.
 */
export const applyUpdate = (state: TimelineState, update: TimelineUpdate): TimelineState => {
	switch (update.type) {
		case "reset": {
			const items = new Map(update.items.map((item) => [item.id, item]));
			const order = byOrdinal(items);
			return {
				...EMPTY_TIMELINE,
				status: "ready",
				seq: update.seq,
				items,
				order,
				...derive(items, order),
				hasOlder: update.has_older,
			};
		}
		case "events":
			if (state.status !== "ready" || update.seq <= state.seq) return state;
			return applyEvents(state, update.seq, update.events);
		case "older": {
			const page = draft(state.items);
			let added = 0;
			for (const item of update.items) {
				if (page.has(item.id)) continue;
				page.set(item.id, item);
				added++;
			}
			const items = page.done();
			const order = added > 0 ? byOrdinal(items) : state.order;
			return {
				...state,
				items,
				order,
				...(added > 0 ? derive(items, order) : {}),
				hasOlder: update.has_older,
				loadingOlder: false,
				error: null,
			};
		}
	}
};
