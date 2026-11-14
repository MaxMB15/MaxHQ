// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { Editor } from "@tiptap/core";
import Document from "@tiptap/extension-document";
import Paragraph from "@tiptap/extension-paragraph";
import Text from "@tiptap/extension-text";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import {
	type LineSpan,
	LONG_LINES,
	misspellingsOf,
	SPELL_DELAY_MS,
	Spelling,
	switchSpelling,
} from "./spelling";
import type { SpellingChoice } from "./native";
import {
	checkerFor,
	deferred,
	fakeNative,
	installBrowserFakes,
	underlined,
	wordRects,
} from "./testing";

const docFor = (lines: string[]) => ({
	type: "doc",
	content: lines.map((line) => ({
		type: "paragraph",
		...(line === "" ? {} : { content: [{ type: "text", text: line }] }),
	})),
});

let editors: Editor[] = [];

/** A native checker that flags each word in `wrong`. */
const checker = (wrong = ["teh", "wrold", "recieve"]) => {
	const { native } = fakeNative({ spelling: true });
	native.checkSpelling.mockImplementation(checkerFor(wrong));
	return native;
};

const open = (
	lines: string[],
	wrong?: string[],
	options: {
		on?: boolean;
		shown?: () => LineSpan | null;
		/** Another editor's checker, to share it. */
		native?: ReturnType<typeof checker>;
		editable?: boolean;
	} = {}
) => {
	const { native = checker(wrong), editable = true, ...spelling } = options;
	const element = document.createElement("div");
	document.body.append(element);
	const editor = new Editor({
		element,
		editable,
		extensions: [Document, Paragraph, Text, Spelling.configure({ native, ...spelling })],
		content: docFor(lines),
	});
	editors.push(editor);
	return { editor, native };
};

const flush = async () => {
	for (let tick = 0; tick < 10; tick++) await Promise.resolve();
};

/** Runs the timers due in `ms` and lets the checks they start finish. */
const wait = async (ms: number) => {
	await flush();
	await vi.advanceTimersByTimeAsync(ms);
	await flush();
};

/** Types text at a position, the way a key press would. */
const typeAt = (editor: Editor, pos: number, text: string) =>
	editor.view.dispatch(editor.state.tr.insertText(text, pos));

const end = (editor: Editor) => editor.state.doc.content.size - 1;

const words = (editor: Editor) =>
	misspellingsOf(editor.state).map(({ from, to }) => editor.state.doc.textBetween(from, to));

const rightClick = (editor: Editor, x: number, y: number) => {
	const event = new MouseEvent("contextmenu", {
		bubbles: true,
		cancelable: true,
		clientX: x,
		clientY: y,
	});
	editor.view.dom.dispatchEvent(event);
	return event;
};

beforeAll(installBrowserFakes);

beforeEach(() => {
	vi.useFakeTimers();
	wordRects.clear();
});

afterEach(() => {
	for (const editor of editors) if (!editor.isDestroyed) editor.destroy();
	editors = [];
	vi.useRealTimers();
});

describe("spell check", () => {
	it("checks only the prose, skipping inline code, code blocks, links and paths", async () => {
		const { editor, native } = open([
			"Fix teh login",
			"Run `teh --wrold` now",
			"See https://teh.example/wrold",
			"Edit /usr/teh/wrold.rs",
			"```",
			"teh wrold",
			"```",
			"the wrold ends",
		]);
		await wait(0);

		expect(native.checkSpelling).toHaveBeenCalledTimes(1);
		const asked = native.checkSpelling.mock.calls[0]![0];
		expect(asked).not.toContain("--wrold");
		expect(asked).not.toContain("https");
		expect(asked).not.toContain("/usr");
		expect(words(editor)).toEqual(["teh", "wrold"]);
		const [first, last] = misspellingsOf(editor.state);
		expect(editor.state.doc.resolve(first!.from).parent.textContent).toBe("Fix teh login");
		expect(editor.state.doc.resolve(last!.from).parent.textContent).toBe("the wrold ends");
		expect(underlined()).toEqual(["teh", "wrold"]);
	});

	it("checks again 300 ms after typing stops, asking only about changed lines", async () => {
		const { editor, native } = open(["Fix teh login", "Then ship it"]);
		await wait(0);
		expect(SPELL_DELAY_MS).toBe(300);

		typeAt(editor, end(editor), " and recieve");
		await wait(150);
		typeAt(editor, end(editor), " thanks");
		await wait(299);
		expect(native.checkSpelling).toHaveBeenCalledTimes(1);

		await wait(1);
		expect(native.checkSpelling).toHaveBeenCalledTimes(2);
		expect(native.checkSpelling).toHaveBeenLastCalledWith("Then ship it and recieve thanks");
		expect(words(editor)).toEqual(["teh", "recieve"]);
		expect(underlined()).toEqual(["teh", "recieve"]);
	});

	it("keeps an underline on its word as text moves, and drops it once the word changes", async () => {
		const { editor } = open(["Fix teh login"]);
		await wait(0);

		typeAt(editor, 1, "Please ");
		expect(words(editor)).toEqual(["teh"]);
		expect(misspellingsOf(editor.state)[0]!.from).toBe(12);

		typeAt(editor, 15, "e");
		expect(words(editor)).toEqual([]);
	});

	it("drops a check that comes back after the text changed", async () => {
		const { editor, native } = open(["Fix teh login"]);
		const slow = deferred<Awaited<ReturnType<typeof native.checkSpelling>>>();
		native.checkSpelling.mockReturnValueOnce(slow.promise);
		await wait(0);

		typeAt(editor, 1, "Now ");
		slow.resolve([{ start: 4, end: 7, word: "teh", language: "en" }]);
		await wait(0);
		expect(words(editor)).toEqual([]);

		await wait(SPELL_DELAY_MS);
		expect(words(editor)).toEqual(["teh"]);
	});

	it("replaces a word from the native menu", async () => {
		const { editor, native } = open(["Fix teh login"]);
		native.spellingMenu.mockResolvedValue({ kind: "replace", word: "the" });
		await wait(0);
		wordRects.set("teh", { left: 30, top: 0, right: 52, bottom: 18 });

		const event = rightClick(editor, 40, 9);
		await wait(0);

		expect(event.defaultPrevented).toBe(true);
		expect(native.spellingMenu).toHaveBeenCalledWith(
			{ start: 4, end: 7, word: "teh", language: "en" },
			{ x: 40, y: 9 }
		);
		expect(editor.getText()).toBe("Fix the login");
		expect(words(editor)).toEqual([]);
	});

	it("checks again after Ignore Spelling or Learn Spelling, without the word", async () => {
		const { editor, native } = open(["Fix teh login, wrold"]);
		native.spellingMenu.mockResolvedValue({ kind: "learned" });
		await wait(0);
		wordRects.set("teh", { left: 30, top: 0, right: 52, bottom: 18 });

		rightClick(editor, 40, 9);
		await wait(0);

		expect(native.checkSpelling).toHaveBeenCalledTimes(2);
		expect(words(editor)).toEqual(["wrold"]);
		expect(editor.getText()).toBe("Fix teh login, wrold");
	});

	it("opens the menu with no misspelling away from an underline", async () => {
		const { editor, native } = open(["Fix teh login"]);
		await wait(0);

		const event = rightClick(editor, 200, 9);
		await wait(0);

		expect(event.defaultPrevented).toBe(true);
		expect(native.spellingMenu).toHaveBeenCalledWith(null, { x: 200, y: 9 });
		expect(editor.getText()).toBe("Fix teh login");
	});

	it("takes its underlines away when the editor goes", async () => {
		const { editor } = open(["Fix teh login"]);
		await wait(0);
		expect(underlined()).toEqual(["teh"]);

		editor.destroy();
		expect(underlined()).toEqual([]);
	});

	it("checks nothing while off and leaves the right-click to the webview", async () => {
		const { editor, native } = open(["Fix teh login"], undefined, { on: false });
		await wait(SPELL_DELAY_MS);
		expect(native.checkSpelling).not.toHaveBeenCalled();
		expect(underlined()).toEqual([]);

		const event = rightClick(editor, 40, 9);
		await wait(0);
		expect(event.defaultPrevented).toBe(false);
		expect(native.spellingMenu).not.toHaveBeenCalled();
	});

	it("takes the underlines away when switched off and brings them back when switched on", async () => {
		const { editor, native } = open(["Fix teh login"]);
		await wait(0);
		expect(underlined()).toEqual(["teh"]);

		switchSpelling(editor.view, false);
		expect(underlined()).toEqual([]);
		expect(words(editor)).toEqual([]);
		typeAt(editor, end(editor), " wrold");
		await wait(SPELL_DELAY_MS);
		expect(native.checkSpelling).toHaveBeenCalledTimes(1);

		switchSpelling(editor.view, true);
		await wait(0);
		expect(words(editor)).toEqual(["teh", "wrold"]);
	});

	it("checks only the lines on screen and a margin in a long document, more on a scroll, and keeps what it found", async () => {
		const lines = Array.from({ length: LONG_LINES + 100 }, () => "Plain line");
		lines[79] = "Before alpha";
		lines[80] = "First bravo";
		lines[219] = "Last charlie";
		lines[220] = "After delta";
		lines[0] = "Fix teh login";
		lines[150] = "Hello wrold";
		lines[LONG_LINES + 99] = "We recieve it";
		let shown: LineSpan = { from: 140, to: 160 };
		const { editor, native } = open(lines, undefined, { shown: () => shown });
		await wait(0);

		const asked = native.checkSpelling.mock.calls[0]![0];
		expect(asked).toContain("bravo");
		expect(asked).not.toContain("alpha");
		expect(asked).toContain("charlie");
		expect(asked).not.toContain("delta");
		expect(words(editor)).toEqual(["wrold"]);

		shown = { from: LONG_LINES + 80, to: LONG_LINES + 100 };
		document.dispatchEvent(new Event("scroll"));
		await wait(SPELL_DELAY_MS);
		// What it found before stays underlined past the margin.
		expect(words(editor)).toEqual(["wrold", "recieve"]);
		expect(underlined()).toEqual(["wrold", "recieve"]);
	});

	it("doesn't put a long document's check off while something else scrolls", async () => {
		const lines = Array.from({ length: LONG_LINES + 1 }, () => "Plain line");
		const { editor } = open(lines, undefined, { shown: () => ({ from: 0, to: 20 }) });
		const feed = document.createElement("div");
		document.body.append(feed);
		await wait(0);

		typeAt(editor, 1, "Fix teh ");
		for (let tick = 0; tick < 5; tick++) {
			await wait(100);
			feed.dispatchEvent(new Event("scroll"));
		}
		expect(words(editor)).toEqual(["teh"]);
		feed.remove();
	});

	it("checks a line once for every editor on the same checker", async () => {
		const { native } = open(["Fix teh login"]);
		await wait(0);
		const { editor } = open(["Fix teh login", "Then ship it"], undefined, { native });
		await wait(0);

		expect(native.checkSpelling).toHaveBeenCalledTimes(2);
		expect(native.checkSpelling).toHaveBeenLastCalledWith("Then ship it");
		expect(words(editor)).toEqual(["teh"]);
	});

	it("takes a word ignored in one editor away in every editor on the same checker", async () => {
		const { editor: first, native } = open(["Fix teh login, wrold"]);
		const { editor: second } = open(["Say teh again"], undefined, { native });
		native.spellingMenu.mockResolvedValue({ kind: "ignored" });
		await wait(0);
		expect(words(second)).toEqual(["teh"]);
		wordRects.set("teh", { left: 30, top: 0, right: 52, bottom: 18 });

		rightClick(first, 40, 9);
		await wait(0);

		expect(words(first)).toEqual(["wrold"]);
		expect(words(second)).toEqual([]);
		expect(underlined()).toEqual(["wrold"]);
	});

	it("checks text that loads as the editor opens at once, without waiting for a pause", async () => {
		const { editor, native } = open([""]);
		editor.commands.setContent(docFor(["Fix teh login"]));
		await wait(0);

		expect(native.checkSpelling).toHaveBeenCalledWith("Fix teh login");
		expect(words(editor)).toEqual(["teh"]);
	});

	it("leaves the editor alone when a check finds what it shows already", async () => {
		const { editor } = open(["Fix teh login", "Then ship it"]);
		await wait(0);
		const transactions = vi.fn();
		editor.on("transaction", transactions);

		typeAt(editor, end(editor), " now");
		await wait(SPELL_DELAY_MS);

		expect(transactions).toHaveBeenCalledTimes(1);
		expect(words(editor)).toEqual(["teh"]);
	});

	it("checks, draws and changes nothing in an editor you can't type in", async () => {
		const { editor, native } = open(["Fix teh login"], undefined, { editable: false });
		await wait(SPELL_DELAY_MS);
		expect(native.checkSpelling).not.toHaveBeenCalled();
		expect(underlined()).toEqual([]);
		expect(rightClick(editor, 40, 9).defaultPrevented).toBe(false);

		editor.setEditable(true);
		await wait(0);
		expect(underlined()).toEqual(["teh"]);

		// A menu still open when the editor locks replaces nothing.
		const menu = deferred<SpellingChoice>();
		native.spellingMenu.mockReturnValueOnce(menu.promise);
		wordRects.set("teh", { left: 30, top: 0, right: 52, bottom: 18 });
		rightClick(editor, 40, 9);
		editor.setEditable(false);
		expect(underlined()).toEqual([]);
		menu.resolve({ kind: "replace", word: "the" });
		await wait(0);
		expect(editor.getText()).toBe("Fix teh login");
	});
});
