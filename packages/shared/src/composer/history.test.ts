// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: walking back from
// the newest, forward past it, an edited recall, a list that changes under
// the walk and repeats kept once. The draft, the caret, the queue and which
// messages count are MaxHQ's.

import { describe, expect, it } from "vitest";

import type { AttachmentRef, Delivery, TimelineItem } from "../bindings";
import type { Segment } from "../pills/draft";
import { timelineItem } from "../timeline/build";

import {
	type HistoryCaret,
	historyEntries,
	type HistoryPosition,
	type HistoryStep,
	stepHistory,
} from "./history";

const sent = (
	id: string,
	text: string,
	delivery: Delivery = "sent",
	attachments: AttachmentRef[] = []
): TimelineItem => timelineItem({ kind: "user_message", text, attachments, delivery }, { id });

const reply = (id: string, text: string): TimelineItem =>
	timelineItem({ kind: "assistant_message", text, streaming: false }, { id });

const timeline = (...items: TimelineItem[]) => ({
	items: new Map(items.map((item) => [item.id, item])),
	order: items.map((item) => item.id),
});

const typed = (text: string): Segment[] => (text === "" ? [] : [{ type: "text", text }]);

const AT_START: HistoryCaret = { atStart: true, onEdge: true };
const ON_EDGE: HistoryCaret = { atStart: false, onEdge: true };
const INSIDE: HistoryCaret = { atStart: false, onEdge: false };

const entries = historyEntries(
	timeline(sent("m1", "first"), reply("r1", "reply"), sent("m2", "second"), sent("m3", "third"))
);

/** Where the walk is once a step's entry went in, as the composer keeps it. */
const positionOf = (step: HistoryStep | null): HistoryPosition => {
	if (step?.type !== "entry") throw new Error("The step brought back no entry");
	const { entry, draft } = step;
	return { entryId: entry.id, prompt: entry.prompt, recalled: entry.prompt, draft };
};

const step = (
	direction: "backward" | "forward",
	position: HistoryPosition | null,
	text: string,
	caret: HistoryCaret = AT_START,
	queued = false
) =>
	stepHistory({
		direction,
		entries: () => entries,
		position,
		text,
		segments: typed(text),
		caret,
		queued,
	});

const at = (entryId: string, prompt: string, draft = ""): HistoryPosition => ({
	entryId,
	prompt,
	recalled: prompt,
	draft: typed(draft),
});

describe("historyEntries", () => {
	it("lists what you sent, oldest first, keeping a repeat in a row once under the newer", () => {
		const collapsed = historyEntries(
			timeline(
				sent("m1", "same"),
				sent("m2", "same"),
				sent("m3", "other"),
				sent("m4", "same")
			)
		);
		expect(collapsed.map(({ id, prompt }) => ({ id, prompt }))).toEqual([
			{ id: "m2", prompt: "same" },
			{ id: "m3", prompt: "other" },
			{ id: "m4", prompt: "same" },
		]);
	});

	it("keeps steered messages and leaves out queued, cancelled and empty ones", () => {
		const listed = historyEntries(
			timeline(
				sent("m1", "typed"),
				sent("m2", "into the turn", "steer"),
				sent("m3", "waiting", "queued"),
				sent("m4", "taken back", "cancelled"),
				sent("m5", "  \n ")
			)
		);
		expect(listed.map((entry) => entry.id)).toEqual(["m1", "m2"]);
	});

	it("keeps a message with only a file, and tells repeats apart by their files", () => {
		const file = (name: string): AttachmentRef => ({
			id: `att-${name}`,
			name,
			kind: "file",
			bytes: 10,
			mime: null,
			path: `/staged/${name}`,
			thumbnail: null,
		});
		const listed = historyEntries(
			timeline(
				sent("m1", "", "sent", [file("a.log")]),
				sent("m2", "", "sent", [file("b.log")])
			)
		);
		expect(listed.map((entry) => entry.prompt)).toEqual(["Attached: a.log", "Attached: b.log"]);
	});

	it("is empty for a timeline with nothing sent", () => {
		expect(historyEntries(timeline(reply("r1", "hello")))).toEqual([]);
	});
});

describe("stepHistory", () => {
	it("walks back from the newest entry", () => {
		const first = step("backward", null, "");
		expect(first).toMatchObject({ type: "entry", entry: { id: "m3", prompt: "third" } });
		expect(step("backward", positionOf(first), "third", ON_EDGE)).toMatchObject({
			entry: { prompt: "second" },
		});
	});

	it("stops at the oldest entry", () => {
		expect(step("backward", at("m1", "first"), "first", ON_EDGE)).toBeNull();
	});

	it("walks forward and puts the empty draft back past the newest entry", () => {
		const newer = step("forward", at("m2", "second"), "second", ON_EDGE);
		expect(newer).toMatchObject({ entry: { prompt: "third" } });
		expect(step("forward", positionOf(newer), "third", ON_EDGE)).toEqual({
			type: "draft",
			draft: [],
		});
	});

	it("keeps the draft from the start of the walk and puts it back with Down", () => {
		const draft = typed("half a thought");
		const first = stepHistory({
			direction: "backward",
			entries: () => entries,
			position: null,
			text: "half a thought",
			segments: draft,
			caret: AT_START,
			queued: false,
		});
		expect(first).toMatchObject({ entry: { prompt: "third" }, draft });
		const older = step("backward", positionOf(first), "third", ON_EDGE);
		expect(older).toMatchObject({ entry: { prompt: "second" }, draft });
		const newer = step("forward", positionOf(older), "second", ON_EDGE);
		expect(step("forward", positionOf(newer), "third", ON_EDGE)).toEqual({
			type: "draft",
			draft,
		});
	});

	it("starts only from the very start of the box, and never with Down", () => {
		expect(step("backward", null, "half a thought", ON_EDGE)).toBeNull();
		expect(step("backward", null, "half a thought", INSIDE)).toBeNull();
		expect(step("forward", null, "", AT_START)).toBeNull();
	});

	it("reads the entries only once the caret allows a step", () => {
		let reads = 0;
		const counted = (caret: HistoryCaret) =>
			stepHistory({
				direction: "backward",
				entries: () => {
					reads += 1;
					return entries;
				},
				position: null,
				text: "half a thought",
				segments: typed("half a thought"),
				caret,
				queued: false,
			});
		expect(counted(INSIDE)).toBeNull();
		expect(reads).toBe(0);
		expect(counted(AT_START)).not.toBeNull();
		expect(reads).toBe(1);
	});

	it("goes on only while the caret is on the first or last line", () => {
		expect(step("backward", at("m3", "third"), "third", INSIDE)).toBeNull();
		expect(step("forward", at("m2", "second"), "second", INSIDE)).toBeNull();
	});

	it("waits for the queue before a walk starts, but not once it has", () => {
		expect(step("backward", null, "", AT_START, true)).toBeNull();
		expect(step("backward", at("m3", "third"), "third", ON_EDGE, true)).toMatchObject({
			entry: { prompt: "second" },
		});
	});

	it("treats an edited recall as a fresh draft", () => {
		const position = at("m3", "third");
		expect(step("backward", position, "third edited", ON_EDGE)).toBeNull();
		expect(step("forward", position, "third edited", ON_EDGE)).toBeNull();
		expect(step("backward", position, "third edited", AT_START)).toMatchObject({
			entry: { id: "m3" },
			draft: typed("third edited"),
		});
	});

	it("follows the entry by id when the list changes under it", () => {
		const grown = historyEntries(
			timeline(sent("m0", "zeroth"), sent("m1", "A"), sent("m2", "B"), sent("m3", "A"))
		);
		const older = stepHistory({
			direction: "backward",
			entries: () => grown,
			position: at("m1", "A"),
			text: "A",
			segments: typed("A"),
			caret: ON_EDGE,
			queued: false,
		});
		expect(older).toMatchObject({ entry: { prompt: "zeroth" } });
		const missing = stepHistory({
			direction: "forward",
			entries: () => grown,
			position: at("gone", "not sent"),
			text: "not sent",
			segments: typed("not sent"),
			caret: ON_EDGE,
			queued: false,
		});
		expect(missing).toBeNull();
	});

	it("falls back to matching text when a repeat takes the entry's place", () => {
		const collapsed = historyEntries(timeline(sent("m1", "first"), sent("m3", "A")));
		const older = stepHistory({
			direction: "backward",
			entries: () => collapsed,
			position: at("m2", "A"),
			text: "A",
			segments: typed("A"),
			caret: ON_EDGE,
			queued: false,
		});
		expect(older).toMatchObject({ entry: { prompt: "first" } });
	});

	it("does nothing with no history", () => {
		const none = stepHistory({
			direction: "backward",
			entries: () => [],
			position: null,
			text: "",
			segments: [],
			caret: AT_START,
			queued: false,
		});
		expect(none).toBeNull();
	});
});
