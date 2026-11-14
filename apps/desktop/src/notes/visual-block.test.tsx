// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { createRef, useEffect } from "react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { editorOf, installBrowserFakes } from "@/composer/testing";
import { MAX_NOTE_BODY } from "@maxhq/shared/notes/visuals";
import { toVisualFence } from "@maxhq/shared/visuals/fence";
import { VISUAL_MIME, writeVisualTransfer } from "@maxhq/shared/visuals/transfer";
import { MAX_VISUAL_BYTES } from "@maxhq/shared/visuals/visual";

import { NoteEditor, type NoteEditorHandle } from "./editor";

// Each frame mounted, with the page it runs, so a test can tell a card
// was drawn once and kept.
const frames = vi.hoisted(() => ({ mounted: [] as string[] }));
vi.mock("@/visuals/visual-frame", () => ({
	VisualFrame: ({ html }: { html: string }) => {
		useEffect(() => {
			frames.mounted.push(html);
			// Counts mounts only, so a page that changes in place isn't one.
			// eslint-disable-next-line react-hooks/exhaustive-deps
		}, []);
		return <div data-testid="frame">{html.slice(0, 40)}</div>;
	},
}));

beforeAll(installBrowserFakes);
beforeEach(() => {
	frames.mounted = [];
});

const FENCE = toVisualFence({ html: "<p>hi</p>", title: "Chart" });
// intro, blank, the fence's three lines, blank, after.
const NOTE = `intro\n\n${FENCE}\n\nafter`;

const renderNote = async (initial: string, { readOnly = false } = {}) => {
	const handle = createRef<NoteEditorHandle>();
	const onChange = vi.fn<(text: string) => void>();
	const onRefuse = vi.fn<(why: string) => void>();
	const view = render(
		<NoteEditor
			ref={handle}
			initial={initial}
			readOnly={readOnly}
			onChange={onChange}
			onRefuse={onRefuse}
		/>
	);
	const textbox = within(view.container).getByRole("textbox", { name: "Note" });
	const editor = editorOf(textbox);
	// The cards draw a moment after the editor does.
	await act(async () => {});
	return { handle: handle.current!, onChange, onRefuse, textbox, editor, view };
};

/** The document position of the start of the `index`th line. */
const lineStart = (text: string, index: number) => {
	const lines = text.split("\n");
	let pos = 1;
	for (let at = 0; at < index; at++) pos += lines[at]!.length + 2;
	return pos;
};

/** The block's lines as a document range in `NOTE`. */
const BLOCK = { from: lineStart(NOTE, 2), to: lineStart(NOTE, 4) + 3 };

const card = () => screen.queryByRole("figure", { name: "Chart" });

const settle = () => act(async () => {});

const paste = (textbox: HTMLElement, types: Record<string, string>) =>
	fireEvent.paste(textbox, {
		clipboardData: {
			getData: (type: string) => types[type] ?? "",
			types: Object.keys(types),
			files: [],
		},
	});

describe("a visual block in a note", () => {
	it("draws a closed fence as the visual's card, its lines hidden", async () => {
		const { textbox } = await renderNote(NOTE);
		expect(card()).toBeInTheDocument();
		expect(frames.mounted).toEqual(["<p>hi</p>"]);
		const hidden = [...textbox.querySelectorAll("p.md-preview-source")].map(
			(line) => line.textContent
		);
		expect(hidden).toEqual(FENCE.split("\n"));
	});

	it("shows an open fence as text", async () => {
		const { textbox } = await renderNote('intro\n\n```html visual title="Chart"\n<p>hi</p>');
		expect(card()).toBeNull();
		expect(textbox.querySelector("p.md-preview-source")).toBeNull();
		expect(textbox).toHaveTextContent("<p>hi</p>");
	});

	it("keeps its card as you type elsewhere, however large it is", async () => {
		const html = `<p>${"a".repeat(400 * 1024)}</p>`;
		const text = `intro\n\n${toVisualFence({ html, title: "Big" })}\n`;
		const { editor } = await renderNote(text);
		const figure = screen.getByRole("figure", { name: "Big" });
		expect(frames.mounted).toHaveLength(1);
		act(() => editor.view.focus());
		for (const [at, key] of [..."typing"].entries()) {
			act(
				() =>
					void editor
						.chain()
						.setTextSelection(6 + at)
						.insertContent(key)
						.run()
			);
			await settle();
		}
		expect(editor.state.doc.child(0).textContent).toBe("introtyping");
		expect(frames.mounted).toHaveLength(1);
		expect(screen.getByRole("figure", { name: "Big" })).toBe(figure);
	});

	it("shows the source on </> until the caret leaves the block", async () => {
		const { textbox, editor } = await renderNote(NOTE);
		act(() => editor.view.focus());
		fireEvent.click(screen.getByRole("button", { name: "Edit the source" }));
		await settle();
		expect(card()).toBeNull();
		expect(textbox.querySelector("p.md-preview-source")).toBeNull();
		expect(editor.state.selection.$head.index(0)).toBe(3);

		// Moving about inside the block keeps it open.
		act(() => void editor.commands.setTextSelection(BLOCK.from));
		await settle();
		expect(card()).toBeNull();

		act(() => void editor.commands.setTextSelection(lineStart(NOTE, 6)));
		await settle();
		expect(card()).toBeInTheDocument();
	});

	it("has no </> when read-only", async () => {
		await renderNote(NOTE, { readOnly: true });
		await settle();
		expect(card()).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Edit the source" })).toBeNull();
	});

	it("opens as the caret types its closing fence", async () => {
		const { editor } = await renderNote('intro\n```html visual title="Chart"\n<p>hi</p>\n');
		act(() => editor.view.focus());
		const end = editor.state.doc.content.size - 1;
		act(() => void editor.chain().setTextSelection(end).insertContent("```").run());
		await settle();
		expect(card()).toBeNull();
		act(() => void editor.commands.setTextSelection(1));
		await settle();
		expect(card()).toBeInTheDocument();
	});

	it("steps over the block with the arrows, as one unit", async () => {
		const { textbox, editor } = await renderNote(NOTE);
		editor.view.endOfTextblock = () => true;
		act(() => editor.view.focus());
		const selection = () => [editor.state.selection.from, editor.state.selection.to];

		act(() => void editor.commands.setTextSelection(lineStart(NOTE, 1)));
		fireEvent.keyDown(textbox, { key: "ArrowDown" });
		expect(selection()).toEqual([BLOCK.from, BLOCK.to]);
		await settle();
		expect(card()).toBeInTheDocument();
		expect(textbox.querySelector(".md-visual")).toHaveAttribute("data-selected");

		fireEvent.keyDown(textbox, { key: "ArrowDown" });
		expect(selection()).toEqual([lineStart(NOTE, 5), lineStart(NOTE, 5)]);
		expect(textbox.querySelector(".md-visual")).not.toHaveAttribute("data-selected");

		fireEvent.keyDown(textbox, { key: "ArrowLeft" });
		expect(selection()).toEqual([BLOCK.from, BLOCK.to]);
		fireEvent.keyDown(textbox, { key: "ArrowUp" });
		expect(selection()).toEqual([lineStart(NOTE, 1), lineStart(NOTE, 1)]);
		await settle();
		expect(card()).toBeInTheDocument();
	});

	it("opens the selected block's source on Enter", async () => {
		const { textbox, editor } = await renderNote(NOTE);
		editor.view.endOfTextblock = () => true;
		act(() => editor.view.focus());
		act(() => void editor.commands.setTextSelection(lineStart(NOTE, 1)));
		fireEvent.keyDown(textbox, { key: "ArrowDown" });
		fireEvent.keyDown(textbox, { key: "Enter" });
		await settle();
		expect(card()).toBeNull();
		expect(editor.state.selection.$head.index(0)).toBe(3);
	});

	it("selects the block on Backspace after it, and deletes it on the next", async () => {
		const { textbox, editor, handle } = await renderNote(NOTE);
		act(() => editor.view.focus());
		act(() => void editor.commands.setTextSelection(lineStart(NOTE, 5)));
		fireEvent.keyDown(textbox, { key: "Backspace" });
		expect([editor.state.selection.from, editor.state.selection.to]).toEqual([
			BLOCK.from,
			BLOCK.to,
		]);
		act(() => void editor.commands.deleteSelection());
		expect(handle.text()).toBe("intro\n\n\n\nafter");
	});

	it("selects the whole block from its title row, for Delete, Copy and Cut", async () => {
		const { textbox, editor, handle } = await renderNote(NOTE);
		fireEvent.mouseDown(within(card()!).getByText("Chart"));
		expect([editor.state.selection.from, editor.state.selection.to]).toEqual([
			BLOCK.from,
			BLOCK.to,
		]);
		expect(textbox.querySelector(".md-visual")).toHaveAttribute("data-selected");

		const copied: Record<string, string> = {};
		const clipboardData = {
			setData: (type: string, value: string) => {
				copied[type] = value;
			},
			clearData: () => {},
			getData: () => "",
		};
		fireEvent.copy(textbox, { clipboardData });
		expect(copied["text/plain"]).toBe(FENCE);

		fireEvent.cut(textbox, { clipboardData });
		expect(handle.text()).toBe("intro\n\n\n\nafter");
		await settle();
		expect(card()).toBeNull();
	});

	it("leaves a press in the page to the page", async () => {
		const { editor } = await renderNote(NOTE);
		const before = editor.state.selection.from;
		fireEvent.mouseDown(screen.getByTestId("frame"));
		expect(editor.state.selection.from).toBe(before);
	});

	it("redraws a visual a change from elsewhere puts in", async () => {
		const { handle } = await renderNote("intro\n");
		const other = toVisualFence({ html: "<p>other</p>", title: "Chart" });
		act(() => handle.applyIncoming(`intro\n\n${other}\n`, "intro\n"));
		await settle();
		expect(card()).toBeInTheDocument();
		expect(frames.mounted).toEqual(["<p>other</p>"]);
	});
});

describe("a visual pasted or dropped into a note", () => {
	const transfer = (html = "<p>new</p>") =>
		writeVisualTransfer({ html, title: "New", from: "Atlas on devbox" });
	const NEW = toVisualFence({ html: "<p>new</p>", title: "New" });

	it("goes in at the caret as its block, on lines of its own", async () => {
		const { textbox, editor, handle } = await renderNote("one\ntwo");
		act(() => editor.view.focus());
		act(() => void editor.commands.setTextSelection(4));
		paste(textbox, { [VISUAL_MIME]: transfer(), "text/plain": "<p>new</p>" });
		expect(handle.text()).toBe(`one\n\n${NEW}\n\ntwo`);
		// The caret waits on the blank line after it, so the card shows.
		expect(editor.state.selection.$head.index(0)).toBe(5);
		await settle();
		expect(screen.getByRole("figure", { name: "New" })).toBeInTheDocument();
	});

	it("goes in as one undo step", async () => {
		const { textbox, editor, handle } = await renderNote("one");
		act(() => editor.view.focus());
		act(() => void editor.commands.setTextSelection(4));
		paste(textbox, { [VISUAL_MIME]: transfer() });
		act(() => void editor.commands.undo());
		expect(handle.text()).toBe("one");
	});

	it("says why a visual too large stays out", async () => {
		const { textbox, onRefuse, handle } = await renderNote("one");
		paste(textbox, { [VISUAL_MIME]: transfer("a".repeat(MAX_VISUAL_BYTES + 1)) });
		expect(onRefuse).toHaveBeenCalledWith(
			"This visual is 513 KB, and a visual holds at most 512 KB."
		);
		expect(handle.text()).toBe("one");
	});

	it("says why a note would grow too large", async () => {
		const big = "a".repeat(MAX_NOTE_BODY - 10);
		const { textbox, onRefuse, handle } = await renderNote(big);
		paste(textbox, { [VISUAL_MIME]: transfer() });
		expect(onRefuse).toHaveBeenCalledWith(
			"This would make the note 1.0 MB, and a note holds at most 1 MB."
		);
		expect(handle.text()).toBe(big);
	});

	it("leaves other pastes as plain lines", async () => {
		const { textbox, editor, handle } = await renderNote("");
		act(() => editor.view.focus());
		paste(textbox, { "text/plain": "a\nb" });
		expect(handle.text()).toBe("a\nb");
	});

	it("goes in where it's dropped, past a visual it lands in", async () => {
		const { textbox, editor, handle } = await renderNote(NOTE);
		editor.view.posAtCoords = () => ({ pos: lineStart(NOTE, 3), inside: -1 });
		fireEvent.drop(textbox, {
			clientX: 10,
			clientY: 10,
			dataTransfer: {
				getData: (type: string) => (type === VISUAL_MIME ? transfer() : ""),
				types: [VISUAL_MIME],
				files: [],
			},
		});
		expect(handle.text()).toBe(`intro\n\n${FENCE}\n\n${NEW}\n\nafter`);
	});

	it("puts in and takes out a visual through its handle", async () => {
		const { editor, handle } = await renderNote("one\ntwo");
		act(() => void editor.commands.setTextSelection(4));
		let placed: ReturnType<NoteEditorHandle["insertVisual"]> = { refused: "" };
		act(() => {
			placed = handle.insertVisual({ html: "<p>new</p>", title: "New" });
		});
		expect(handle.text()).toBe(`one\n\n${NEW}\n\ntwo`);
		if ("refused" in placed) throw new Error("refused");
		act(() => void editor.chain().setTextSelection(1).insertContent("zero ").run());
		let taken = false;
		act(() => {
			taken = handle.removePlaced(placed as Exclude<typeof placed, { refused: string }>);
		});
		expect(taken).toBe(true);
		expect(handle.text()).toBe("zero one\ntwo");
	});
});
