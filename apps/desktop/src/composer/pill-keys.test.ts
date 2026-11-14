// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { getSchema } from "@tiptap/core";
import Document from "@tiptap/extension-document";
import Paragraph from "@tiptap/extension-paragraph";
import Text from "@tiptap/extension-text";
import { history, undo } from "@tiptap/pm/history";
import { EditorState, TextSelection } from "@tiptap/pm/state";
import { describe, expect, it, vi } from "vitest";

import { DRAFT_CLIPBOARD_MIME } from "@maxhq/shared/pills/clipboard";
import { OBJECT_MARK as MARK, type Segment } from "@maxhq/shared/pills/draft";

import { markedTextOf, paragraphsFor, segmentsOf } from "./doc";
import { PILL_EXTENSIONS } from "./pill-nodes";
import { deletePill, spaced, stepOverPill, writeDraftClipboard } from "./pill-keys";
import { heldFileKeys, releasedFiles } from "./retained";

const schema = getSchema([Document, Paragraph, Text, ...PILL_EXTENSIONS]);

const file = (key: string): Segment => ({ type: "pill", pill: { type: "attachment", key } });
const text = (value: string): Segment => ({ type: "text", text: value });

/** A state holding the segments, with the caret at `caret` in the draft's text. */
const stateOf = (segments: Segment[], caret: number, plugins = [history()]) => {
	const doc = schema.node("doc", null, paragraphsFor(schema, segments));
	const state = EditorState.create({ schema, doc, plugins });
	return state.apply(state.tr.setSelection(TextSelection.create(state.doc, caret + 1)));
};

describe("arrow keys beside a pill", () => {
	it("step over the pill whole", () => {
		const state = stateOf([text("ab"), file("k1"), text("cd")], 3);
		const back = state.apply(stepOverPill(state, -1)!);
		expect(back.selection.from).toBe(3);
		const on = back.apply(stepOverPill(back, 1)!);
		expect(on.selection.from).toBe(4);
	});

	it("leave the caret to the browser with no pill beside it", () => {
		const state = stateOf([text("ab"), file("k1")], 1);
		expect(stepOverPill(state, -1)).toBeNull();
		expect(stepOverPill(state, 1)).toBeNull();
	});
});

describe("deleting a pill", () => {
	it("takes the pill out whole with Backspace or Delete", () => {
		const state = stateOf([text("fix "), file("k1"), text(" now")], 5);
		const back = state.apply(deletePill(state, -1)!);
		expect(markedTextOf(back.doc)).toBe("fix  now");
		const forward = stateOf([text("fix "), file("k1"), text(" now")], 4);
		expect(markedTextOf(forward.apply(deletePill(forward, 1)!).doc)).toBe("fix  now");
	});

	it("takes the word before it too with Option+Backspace", () => {
		const state = stateOf([text("fix the "), file("k1"), text(" now")], 9);
		expect(markedTextOf(state.apply(deletePill(state, -1, true)!).doc)).toBe("fix  now");
	});

	it("does nothing with no pill beside the caret", () => {
		const state = stateOf([text("fix "), file("k1")], 2);
		expect(deletePill(state, -1)).toBeNull();
	});
});

describe("spacing an inserted pill", () => {
	it("adds a space before a pill that follows a word, and after the last pill", () => {
		expect(spaced([file("k1")], "look", "")).toEqual({
			segments: [text(" "), file("k1"), text(" ")],
			skipSpace: false,
		});
	});

	it("steps over a space that's already there", () => {
		expect(spaced([file("k1")], "look ", " here")).toEqual({
			segments: [file("k1")],
			skipSpace: true,
		});
	});

	it("leaves text alone unless asked", () => {
		expect(spaced([text("#412")], "see ", "").segments).toEqual([text("#412")]);
		expect(spaced([text("#412")], "see ", "", true).segments).toEqual([
			text("#412"),
			text(" "),
		]);
	});
});

describe("copying pills", () => {
	it("writes the plain form, and the fragment in its own type and in HTML", () => {
		const setData = vi.fn();
		writeDraftClipboard({ setData }, "[File: main.rs]", '{"version":1}');
		expect(setData).toHaveBeenCalledWith("text/plain", "[File: main.rs]");
		expect(setData).toHaveBeenCalledWith(DRAFT_CLIPBOARD_MIME, '{"version":1}');
		expect(setData).toHaveBeenCalledWith("text/html", expect.stringContaining("main.rs"));
	});

	it("writes only the plain form when there's no fragment", () => {
		const setData = vi.fn();
		writeDraftClipboard({ setData }, "plain", null);
		expect(setData).toHaveBeenCalledTimes(1);
	});
});

describe("a removed pill's file", () => {
	it("stays while undo could bring the pill back", () => {
		const state = stateOf([text("see "), file("k1")], 5);
		const removed = state.apply(deletePill(state, -1)!);
		expect(segmentsOf(removed.doc)).toEqual([text("see ")]);

		const held = heldFileKeys(removed);
		expect(held).toEqual(new Set(["k1"]));
		expect(releasedFiles({ keys: ["k1"], placed: new Set(), held })).toEqual([]);

		let back = removed;
		undo(removed, (tr) => {
			back = removed.apply(tr);
		});
		expect(markedTextOf(back.doc)).toBe(`see ${MARK}`);
	});

	it("goes once nothing holds it", () => {
		const state = stateOf([text("see "), file("k1"), file("k2")], 6, []);
		const removed = state.apply(deletePill(state, -1)!);
		const held = heldFileKeys(removed);
		expect(held).toEqual(new Set());
		expect(releasedFiles({ keys: ["k1", "k2"], placed: new Set(["k1"]), held })).toEqual([
			"k2",
		]);
	});

	it("stays until the message goes when the history can't be read", () => {
		expect(releasedFiles({ keys: ["k1"], placed: new Set(), held: null })).toEqual([]);
	});
});
