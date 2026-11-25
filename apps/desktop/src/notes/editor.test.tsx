// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, within } from "@testing-library/react";
import { EditorState, type Plugin, TextSelection } from "@tiptap/pm/state";
import type { DecorationSet } from "@tiptap/pm/view";
import { createRef } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { blocksParsed } from "@/composer/markdown";
import { ComposerNativeProvider } from "@/composer/native";
import { SPELL_DELAY_MS } from "@/composer/spelling";
import {
	checkerFor,
	editorOf,
	fakeNative,
	installBrowserFakes,
	underlined,
} from "@/composer/testing";

import { INCOMING_FLASH_MS, NoteEditor, type NoteEditorHandle } from "./editor";

vi.mock("@/agent-view/diagram-render", () => {
	const url = "data:image/svg+xml,drawn";
	const drawn = { state: "drawn", url, width: 200, height: 100, whole: () => url };
	return { drawnDiagram: () => drawn, drawDiagram: () => Promise.resolve(drawn) };
});

beforeAll(installBrowserFakes);
afterEach(() => vi.useRealTimers());

const renderNote = (initial: string, { readOnly = false } = {}) => {
	const handle = createRef<NoteEditorHandle>();
	const onChange = vi.fn<(text: string) => void>();
	const view = render(
		<NoteEditor ref={handle} initial={initial} readOnly={readOnly} onChange={onChange} />
	);
	const textbox = within(view.container).getByRole("textbox", { name: "Note" });
	return { handle: handle.current!, onChange, textbox, editor: editorOf(textbox), view };
};

/** The document position of an offset in the note's text. */
const posAt = (text: string, offset: number) => offset + text.slice(0, offset).split("\n").length;

/** The look's decorations, each as where it is and what it draws. */
const drawnIn = (state: EditorState): string[] => {
	const plugin = state.plugins.find(
		(each) => (each as Plugin & { key: string }).key === "markdownLook$"
	)!;
	const set = plugin.props.decorations!.call(plugin, state) as DecorationSet;
	return set
		.find()
		.map((each) => {
			const spec = each as unknown as {
				type: { attrs?: { class?: string }; spec: { key?: string } };
			};
			return `${each.from}-${each.to}:${spec.type.attrs?.class ?? spec.type.spec.key ?? ""}`;
		})
		.sort();
};

const SAMPLE = [
	"# Title",
	"",
	"Some **bold** text.",
	"",
	"",
	"```js",
	"const a = 1;",
	"",
	"const b = 2;",
	"```",
	"",
	"- [ ] one",
	"- [x] two",
	"",
	"| a | b |",
	"| - | - |",
	"| 1 | 2 |",
	"",
	"$$",
	"x^2",
	"$$",
	"\r odd line ",
	"",
].join("\n");

describe("NoteEditor", () => {
	it("hands back exactly the text it got", () => {
		const { handle, onChange } = renderNote(SAMPLE);
		expect(handle.text()).toBe(SAMPLE);
		expect(onChange).not.toHaveBeenCalled();
		expect(renderNote("").handle.text()).toBe("");
		expect(renderNote("\n\n").handle.text()).toBe("\n\n");
	});

	it("tells of each change with the whole text", () => {
		const { editor, onChange } = renderNote("a\n\nb\n");
		act(
			() =>
				void editor.chain().setTextSelection(posAt("a\n\nb\n", 4)).insertContent("c").run()
		);
		expect(onChange).toHaveBeenLastCalledWith("a\n\nbc\n");
	});

	it("puts in a change from elsewhere around what was typed since, outside undo", () => {
		vi.useFakeTimers();
		const start = "one\ntwo\nthree";
		const { handle, editor, onChange, textbox } = renderNote(start);
		act(
			() =>
				void editor
					.chain()
					.focus()
					.setTextSelection(posAt(start, 3))
					.insertContent("!")
					.run()
		);
		const typed = handle.text();
		expect(typed).toBe("one!\ntwo\nthree");
		// More typing, which the other side hasn't seen.
		act(() => void editor.commands.insertContent("?"));
		const caret = editor.state.selection.head;

		act(() => handle.applyIncoming("one!\nTWO\nthree\nfour", typed));

		expect(handle.text()).toBe("one!?\nTWO\nthree\nfour");
		expect(editor.state.selection.head).toBe(caret);
		expect(onChange).toHaveBeenLastCalledWith("one!?\nTWO\nthree\nfour");
		expect(textbox.querySelectorAll(".note-incoming").length).toBeGreaterThan(0);
		act(() => void vi.advanceTimersByTime(INCOMING_FLASH_MS));
		expect(textbox.querySelector(".note-incoming")).toBeNull();

		// Undo takes back what was typed here, and leaves what came in.
		act(() => void editor.commands.undo());
		expect(handle.text()).toBe("one\nTWO\nthree\nfour");
	});

	it("reads a change from the text it shows when the text it came from is unknown", () => {
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		const { handle } = renderNote("a\nb");
		act(() => handle.applyIncoming("a\nc", "never seen"));
		expect(handle.text()).toBe("a\nc");
		warn.mockRestore();
	});

	it("ticks a task's box in the text, but not when read-only", async () => {
		const text = "- [ ] one\n\nafter";
		const { editor, handle, textbox } = renderNote(text);
		act(() => void editor.commands.setTextSelection(posAt(text, 13)));
		const box = textbox.querySelector<HTMLInputElement>("input.md-task")!;
		expect(box.checked).toBe(false);
		fireEvent.mouseDown(box);
		expect(handle.text()).toBe("- [x] one\n\nafter");
		expect(textbox.querySelector<HTMLInputElement>("input.md-task")?.checked).toBe(true);

		const reading = renderNote("- [ ] one", { readOnly: true });
		// It learns it's read-only a moment after it starts.
		await act(async () => {});
		const shown = reading.textbox.querySelector<HTMLInputElement>("input.md-task")!;
		expect(shown.disabled).toBe(true);
		fireEvent.mouseDown(shown);
		expect(reading.handle.text()).toBe("- [ ] one");
	});

	it("shows a line's markers only while the caret is on it", () => {
		const text = "**a**\n\n**b**";
		const { editor, textbox } = renderNote(text);
		const hidden = () =>
			Array.from(textbox.querySelectorAll(".md-hidden"), (each) => each.textContent);
		act(() => editor.view.focus());
		act(() => void editor.commands.setTextSelection(posAt(text, 0)));
		expect(hidden()).toEqual(["**", "**"]);
		act(() => void editor.commands.setTextSelection(posAt(text, 9)));
		expect(hidden()).toEqual(["**", "**"]);
		act(() => void editor.commands.setTextSelection(posAt(text, 6)));
		expect(hidden()).toEqual(["**", "**", "**", "**"]);
	});

	it("hides the first line's markers while the editor has no focus, caret and all", async () => {
		const text = "# Title\n\nbody";
		const { editor, textbox } = renderNote(text);
		await act(async () => {});
		const hidden = () =>
			Array.from(textbox.querySelectorAll(".md-hidden"), (each) => each.textContent);
		expect(editor.state.selection.from).toBe(1);
		expect(hidden()).toEqual(["# "]);
		act(() => editor.view.focus());
		expect(hidden()).toEqual([]);
		act(() => editor.view.dom.blur());
		expect(hidden()).toEqual(["# "]);
	});

	it("opens a link on a Command-click on a Mac", () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"
		);
		const open = vi.spyOn(window, "open").mockReturnValue(null);
		const text = "see [site](https://example.com) now";
		const { editor, textbox } = renderNote(text);
		const link = posAt(text, 6);
		editor.view.posAtCoords = () => ({ pos: link, inside: -1 });
		fireEvent.mouseDown(textbox, { metaKey: true, button: 0 });
		expect(open).toHaveBeenCalledWith("https://example.com", "_blank", "noreferrer");
		vi.restoreAllMocks();
	});

	it("opens a link on a Control-click off a Mac, and not on the Windows key", () => {
		const open = vi.spyOn(window, "open").mockReturnValue(null);
		const text = "see [site](https://example.com) now";
		const { editor, textbox } = renderNote(text);
		const link = posAt(text, 6);
		editor.view.posAtCoords = () => ({ pos: link, inside: -1 });
		fireEvent.mouseDown(textbox, { metaKey: true, button: 0 });
		expect(open).not.toHaveBeenCalled();
		fireEvent.mouseDown(textbox, { ctrlKey: true, button: 0 });
		expect(open).toHaveBeenCalledWith("https://example.com", "_blank", "noreferrer");
		open.mockRestore();
	});

	it("reads only the block a key touches in a long note, and draws as a fresh read would", () => {
		const blocks = Array.from({ length: 2_000 }, (_, index) =>
			index % 10 === 0
				? `\`\`\`\ncode ${index}\n\n**not bold**\n\`\`\``
				: `## Part ${index}\n\nSome **bold** and \`code\` ${index}.\n- [ ] task ${index}`
		);
		const text = blocks.join("\n\n");
		const { editor } = renderNote(text);
		const middle = posAt(text, text.indexOf("Part 1001") + 4);
		act(() => void editor.commands.setTextSelection(middle));
		for (const key of ["x", "y", " ", "*"]) {
			act(() => void editor.commands.insertContent(key));
			expect(blocksParsed(editor.state)).toBe(1);
		}

		// A blank line joined, a fence opened, then closed again, and a
		// block emptied.
		const at = (find: string) =>
			posAt(
				editor.getText({ blockSeparator: "\n" }),
				editor.getText({ blockSeparator: "\n" }).indexOf(find)
			);
		act(
			() =>
				void editor
					.chain()
					.setTextSelection(at("Part 501") - 3)
					.deleteRange({ from: at("Part 501") - 5, to: at("Part 501") - 3 })
					.run()
		);
		act(() => void editor.chain().setTextSelection(at("Part 702")).insertContent("```").run());
		act(() => void editor.chain().insertContent("x").run());
		act(
			() =>
				void editor
					.chain()
					.setTextSelection(at("Part 702"))
					.deleteRange({ from: at("Part 702"), to: at("Part 702") + 3 })
					.run()
		);

		const fresh = EditorState.create({
			doc: editor.state.doc,
			plugins: editor.state.plugins,
			selection: TextSelection.create(editor.state.doc, editor.state.selection.head),
		});
		expect(drawnIn(editor.state)).toEqual(drawnIn(fresh));
	});

	describe("list keys", () => {
		/** A note with the caret at the "|" in its text, focused. */
		const typing = (marked: string) => {
			const text = marked.replace("|", "");
			const note = renderNote(text);
			act(
				() =>
					void note.editor
						.chain()
						.focus()
						.setTextSelection(posAt(text, marked.indexOf("|")))
						.run()
			);
			const press = (key: string, options: Partial<KeyboardEventInit> = {}) => {
				let passed = true;
				act(() => {
					passed = fireEvent.keyDown(note.textbox, { key, ...options });
				});
				return passed;
			};
			/** The text with the caret marked, or the selection in brackets. */
			const shown = () => {
				const now = note.handle.text();
				const { doc, selection } = note.editor.state;
				const offset = (pos: number) => pos - 1 - doc.resolve(pos).index(0);
				const from = offset(selection.from);
				const to = offset(selection.to);
				if (selection.empty) return `${now.slice(0, from)}|${now.slice(from)}`;
				return `${now.slice(0, from)}[${now.slice(from, to)}]${now.slice(to)}`;
			};
			return { ...note, press, shown };
		};

		it("starts the next item on Enter, and ends the list on an empty one", () => {
			const { press, shown, onChange } = typing("- [x] one|");
			expect(press("Enter")).toBe(false);
			expect(shown()).toBe("- [x] one\n- [ ] |");
			expect(onChange).toHaveBeenLastCalledWith("- [x] one\n- [ ] ");
			press("Enter");
			expect(shown()).toBe("- [x] one\n|");
		});

		it("counts on a numbered item and moves the text after the caret", () => {
			const { press, shown } = typing("> 1. ab|cd");
			press("Enter");
			expect(shown()).toBe("> 1. ab\n> 2. |cd");
		});

		it("moves an empty nested item out a level on Enter", () => {
			const { press, shown } = typing("- a\n  - b\n  - |");
			press("Enter");
			expect(shown()).toBe("- a\n  - b\n- |");
		});

		it("breaks the line plainly off a list, on Shift-Enter, in code and while composing", () => {
			const plain = typing("ab|cd");
			plain.press("Enter");
			expect(plain.shown()).toBe("ab\n|cd");

			const shifted = typing("- ab|");
			expect(shifted.press("Enter", { shiftKey: true })).toBe(false);
			expect(shifted.shown()).toBe("- ab\n|");

			const code = typing("```\n- a|\n```");
			code.press("Enter");
			expect(code.shown()).toBe("```\n- a\n|\n```");

			const composing = typing("- a|");
			composing.press("Enter", { isComposing: true });
			expect(composing.handle.text()).not.toContain("\n- ");
		});

		it("replaces a selection before going on to the next item", () => {
			const text = "- abcd";
			const { editor, press, shown } = typing(text);
			act(
				() =>
					void editor.commands.setTextSelection({
						from: posAt(text, 3),
						to: posAt(text, 5),
					})
			);
			press("Enter");
			expect(shown()).toBe("- a\n- |d");
		});

		it("nests an item on Tab and moves it back out on Shift-Tab", () => {
			const { press, shown } = typing("1. a\n2. b|");
			expect(press("Tab")).toBe(false);
			expect(shown()).toBe("1. a\n   2. b|");
			expect(press("Tab", { shiftKey: true })).toBe(false);
			expect(shown()).toBe("1. a\n2. b|");
			// Already at the edge, the key is still the list's.
			expect(press("Tab", { shiftKey: true })).toBe(false);
			expect(shown()).toBe("1. a\n2. b|");
		});

		it("moves every line of a selection on Tab", () => {
			const text = "- a\n- b\n  - c";
			const { editor, press, shown } = typing(text);
			act(
				() =>
					void editor.commands.setTextSelection({
						from: posAt(text, 4),
						to: posAt(text, text.length),
					})
			);
			press("Tab");
			expect(shown()).toBe("- a\n[  - b\n    - c]");
		});

		it("leaves Tab to move focus off a list", () => {
			const { press, handle } = typing("plain|");
			expect(press("Tab")).toBe(true);
			expect(press("Tab", { shiftKey: true })).toBe(true);
			expect(handle.text()).toBe("plain");
		});

		it("takes each list step back with one undo", () => {
			const { editor, press, shown } = typing("- a|");
			press("Enter");
			press("Tab");
			expect(shown()).toBe("- a\n  - |");
			act(() => void editor.commands.undo());
			expect(shown()).toBe("- a\n- |");
			act(() => void editor.commands.undo());
			expect(shown()).toBe("- a|");
		});
	});

	describe("line keys", () => {
		/** A note with the caret at "|", or the selection in brackets, focused. */
		const selecting = (marked: string, options: { readOnly?: boolean } = {}) => {
			const text = marked.replace(/[|[\]]/g, "");
			const note = renderNote(text, options);
			const caret = marked.indexOf("|");
			const from = caret >= 0 ? caret : marked.indexOf("[");
			const to = caret >= 0 ? caret : marked.indexOf("]") - 1;
			act(
				() =>
					void note.editor
						.chain()
						.focus()
						.setTextSelection({ from: posAt(text, from), to: posAt(text, to) })
						.run()
			);
			const press = (key: string, options: Partial<KeyboardEventInit> = {}) => {
				let passed = true;
				act(() => {
					passed = fireEvent.keyDown(note.textbox, { key, altKey: true, ...options });
				});
				return passed;
			};
			const shown = () => {
				const now = note.handle.text();
				const { doc, selection } = note.editor.state;
				const offset = (pos: number) => pos - 1 - doc.resolve(pos).index(0);
				const start = offset(selection.from);
				const end = offset(selection.to);
				if (selection.empty) return `${now.slice(0, start)}|${now.slice(start)}`;
				return `${now.slice(0, start)}[${now.slice(start, end)}]${now.slice(end)}`;
			};
			return { ...note, press, shown };
		};

		it("moves the caret's line down and up on Option and an arrow", () => {
			const { press, shown, onChange } = selecting("one\ntw|o\nthree");
			expect(press("ArrowDown")).toBe(false);
			expect(shown()).toBe("one\nthree\ntw|o");
			expect(onChange).toHaveBeenLastCalledWith("one\nthree\ntwo");
			press("ArrowUp");
			press("ArrowUp");
			expect(shown()).toBe("tw|o\none\nthree");
		});

		it("moves every line a selection touches, keeping it selected", () => {
			const { press, shown } = selecting("a\nb[b\nc]c\nd");
			press("ArrowDown");
			expect(shown()).toBe("a\nd\nb[b\nc]c");
		});

		it("copies the lines down or up on Shift-Option and an arrow", () => {
			const { press, shown } = selecting("a\nb|b\nc");
			press("ArrowDown", { shiftKey: true });
			expect(shown()).toBe("a\nbb\nb|b\nc");
			press("ArrowUp", { shiftKey: true });
			expect(shown()).toBe("a\nbb\nb|b\nbb\nc");
		});

		it("does nothing past the first or last line, and keeps the key from the webview", () => {
			const { press, shown, onChange } = selecting("o|ne\ntwo");
			expect(press("ArrowUp")).toBe(false);
			expect(shown()).toBe("o|ne\ntwo");
			const last = selecting("one\ntw|o");
			last.press("ArrowDown");
			expect(last.shown()).toBe("one\ntw|o");
			expect(onChange).not.toHaveBeenCalled();
			expect(last.onChange).not.toHaveBeenCalled();
		});

		it("moves lines inside lists and code blocks as plain text", () => {
			const { press, shown } = selecting("- one\n- tw|o\n```\nx = 1\n```");
			press("ArrowUp");
			expect(shown()).toBe("- tw|o\n- one\n```\nx = 1\n```");
			const code = selecting("```\nx = |1\ny = 2\n```");
			code.press("ArrowDown");
			expect(code.shown()).toBe("```\ny = 2\nx = |1\n```");
		});

		it("takes each move back with one undo", () => {
			const { editor, press, shown } = selecting("a|\nb\nc");
			press("ArrowDown");
			press("ArrowDown");
			expect(shown()).toBe("b\nc\na|");
			act(() => void editor.commands.undo());
			expect(shown()).toBe("b\na|\nc");
			act(() => void editor.commands.undo());
			expect(shown()).toBe("a|\nb\nc");
		});

		it("leaves Command-Option arrows to the panes, and does nothing read-only", () => {
			const { press, shown } = selecting("a|\nb");
			expect(press("ArrowDown", { metaKey: true })).toBe(true);
			expect(shown()).toBe("a|\nb");
			const locked = selecting("a|\nb", { readOnly: true });
			locked.press("ArrowDown");
			expect(locked.handle.text()).toBe("a\nb");
		});
	});
});

describe("NoteEditor spelling", () => {
	const WRONG = [
		"recieve",
		"wrold",
		"itme",
		"Heding",
		"wrods",
		"lnik",
		"tommorow",
		"Tpyo",
		"tpyo",
	];

	/** A note on a Mac with its own spell check, which flags each word in `wrong`. */
	const renderChecked = (initial: string, { readOnly = false } = {}) => {
		const { native } = fakeNative({ spelling: true });
		native.checkSpelling.mockImplementation(checkerFor(WRONG));
		const tree = (locked: boolean) => (
			<ComposerNativeProvider value={native}>
				<NoteEditor initial={initial} readOnly={locked} onChange={() => {}} />
			</ComposerNativeProvider>
		);
		const view = render(tree(readOnly));
		const textbox = within(view.container).getByRole("textbox", { name: "Note" });
		return {
			native,
			editor: editorOf(textbox),
			lock: (locked: boolean) => view.rerender(tree(locked)),
		};
	};

	/** Runs the timers due in `ms` and lets the checks they start finish. */
	const wait = (ms: number) => act(async () => void (await vi.advanceTimersByTimeAsync(ms)));

	beforeEach(() => vi.useFakeTimers());

	it("underlines just the misspelled words around tasks, lists, headings, bold and links", async () => {
		renderChecked(
			[
				"- [x] Right click notes to create notes",
				"- [x] Ship the recieve fix",
				"  - [ ] Nested itme here",
				"- [ ] wrold peace",
				"# Heding",
				"**Bold** wrods and [a lnik](https://example.com/tpyo)",
				"Café 😀 naïve tommorow",
				"Tpyo first, then the rest",
				"The rest, then a tpyo",
			].join("\n")
		);
		// The text goes in as the note opens, and is checked at once.
		await wait(0);

		expect(underlined()).toEqual([
			"recieve",
			"itme",
			"wrold",
			"Heding",
			"wrods",
			"lnik",
			"tommorow",
			"Tpyo",
			"tpyo",
		]);
	});

	it("leaves the word you're typing unmarked until you move on", async () => {
		const { editor } = renderChecked("- [ ] Ship it");
		await wait(0);

		act(
			() =>
				void editor
					.chain()
					.focus("end")
					.insertContent({ type: "text", text: " tommorow" })
					.run()
		);
		await wait(SPELL_DELAY_MS);
		expect(underlined()).toEqual([]);

		act(() => void editor.commands.insertContent({ type: "text", text: " or" }));
		await wait(SPELL_DELAY_MS);
		expect(underlined()).toEqual(["tommorow"]);
	});

	it("checks nothing in a read-only note, and checks once it can be typed in", async () => {
		const { native, lock } = renderChecked("Ship the recieve fix", { readOnly: true });
		await wait(SPELL_DELAY_MS);
		expect(native.checkSpelling).not.toHaveBeenCalled();
		expect(underlined()).toEqual([]);

		lock(false);
		await wait(0);
		expect(underlined()).toEqual(["recieve"]);

		lock(true);
		expect(underlined()).toEqual([]);
	});
});
