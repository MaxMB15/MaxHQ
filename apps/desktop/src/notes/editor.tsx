// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project note's text box. It's the composer's kind of editor, one
// paragraph per line, and draws its Markdown the same way, with more of it
// on: markers show only on the caret's line, a task's box ticks on a click,
// Command-click opens a link, and a picture on a line of its own draws.
//
// The text it hands out is exactly the text it got, blank lines, a last line
// break and all. A change from elsewhere comes in as a new text and the text
// it was made from. The difference goes in where it lands after what was
// typed here since, outside undo, with the caret left where it was, and
// flashes for a moment so it's seen.
//
// Enter on a list item starts the next one, or ends the list on an empty
// item, and Tab and Shift-Tab move list items in and out. The text work is
// in the shared notes lists module; here the keys only hand it over. In a
// code block, and anywhere else, Enter and Shift-Enter break the line, and
// Tab off a list moves focus as usual. Option-Up and Option-Down move the
// selection's lines, and with Shift copy them, from the shared notes module
// too.
//
// Spelling checks the composer's way, by the typing settings.
//
// An `html visual` fenced block draws as the visual's card, from
// `./visual-block`. A visual pasted or dropped in from the app goes in as
// that block, on lines of its own, and one too large is refused in words
// through `onRefuse`.

import { t as translate, useI18n } from "@maxhq/shared/i18n";

import "@/composer/composer.css";
import "./editor.css";

import { applyReplacements, replacementsBetween } from "@maxhq/shared/notes/diff";
import { continueList, type ListEdit, shiftList } from "@maxhq/shared/notes/lists";
import { copyLines, moveLines } from "@maxhq/shared/notes/move-lines";
import { type Placed, placedAt, placeVisual, transferredVisual } from "@maxhq/shared/notes/visuals";
import type { Visual } from "@maxhq/shared/visuals/fence";
import Document from "@tiptap/extension-document";
import Paragraph from "@tiptap/extension-paragraph";
import Text from "@tiptap/extension-text";
import { UndoRedo } from "@tiptap/extensions";
import { Extension } from "@tiptap/core";
import { splitBlock } from "@tiptap/pm/commands";
import { closeHistory } from "@tiptap/pm/history";
import { Fragment, type Node as ProseNode, type Schema, Slice } from "@tiptap/pm/model";
import { Plugin, PluginKey, TextSelection } from "@tiptap/pm/state";
import { Mapping, type StepMap } from "@tiptap/pm/transform";
import { Decoration, DecorationSet, type EditorView } from "@tiptap/pm/view";
import { EditorContent, useEditor } from "@tiptap/react";
import {
	forwardRef,
	useEffect,
	useImperativeHandle,
	useLayoutEffect,
	useRef,
	useState,
} from "react";

import { CodeColours } from "@/composer/code-colours";
import { useComposerNative } from "@/composer/native";
import { Spelling } from "@/composer/spelling";
import { useEditorTyping, useSpellingSwitch } from "@/composer/typing-prefs";
import { markedTextOf } from "@/composer/doc";
import {
	enterAddsLine,
	MarkdownLook,
	type MarkdownLookOptions,
	visualLinesAt,
} from "@/composer/markdown";

import { NoteVisualBlocks, useNoteVisuals } from "./visual-block";

export type NoteEditorHandle = {
	/** Applies text computed from `against` without moving the caret, outside undo, and flashes what changed. */
	applyIncoming(text: string, against: string): void;
	text(): string;
	focus(): void;
	/** Puts a visual's block in at the caret, as one undo step, or says why it can't. */
	insertVisual(visual: Visual): Placed | { readonly refused: string };
	/** Takes a block `insertVisual` put in out again. False when it changed since. */
	removePlaced(placed: Placed): boolean;
};

type NoteEditorProps = {
	initial: string;
	readOnly?: boolean;
	onChange(text: string): void;
	onBlur?(): void;
	/** Says why a visual pasted or dropped in couldn't go in. */
	onRefuse?(why: string): void;
};

/** The longest note, past which it stays plain text. */
const NOTE_LOOK: MarkdownLookOptions = {
	maxLength: 1024 * 1024,
	byLine: true,
	tasks: true,
	links: true,
	images: true,
	html: true,
};

/** How long a change from elsewhere stays lit. */
export const INCOMING_FLASH_MS = 1_500;

/** How many texts handed out are remembered, to read a change made from one. */
const KNOWN_TEXTS = 2_000;

/** A text's fingerprint, its length and two hashes, so a long text isn't kept. */
const fingerprint = (text: string): string => {
	let first = 0x811c9dc5;
	let second = 5381;
	for (let index = 0; index < text.length; index++) {
		const code = text.charCodeAt(index);
		first = Math.imul(first ^ code, 0x01000193);
		second = (Math.imul(second, 33) + code) | 0;
	}
	return `${text.length}:${first >>> 0}:${second >>> 0}`;
};

/** A text's lines as paragraphs, every character kept. */
const paragraphsOf = (schema: Schema, text: string): Fragment =>
	Fragment.from(
		text
			.split("\n")
			.map((line) =>
				schema.nodes.paragraph!.create(null, line === "" ? null : schema.text(line))
			)
	);

/** Text as lines to put in at a position. */
const sliceOf = (schema: Schema, text: string): Slice =>
	new Slice(paragraphsOf(schema, text), 1, 1);

/** The document position of each offset in a text, through its line breaks. */
const positionsIn = (text: string): ((offset: number) => number) => {
	const breaks: number[] = [];
	for (let at = text.indexOf("\n"); at !== -1; at = text.indexOf("\n", at + 1)) breaks.push(at);
	return (offset) => {
		let low = 0;
		let high = breaks.length;
		while (low < high) {
			const middle = (low + high) >> 1;
			if (breaks[middle]! < offset) low = middle + 1;
			else high = middle;
		}
		// Each line break before it is two positions, and the text starts at 1.
		return offset + low + 1;
	};
};

/** A document position's offset in the note's text. */
const offsetOf = (doc: ProseNode, pos: number): number => {
	const $pos = doc.resolve(pos);
	const index = $pos.index(0);
	let offset = 0;
	for (let line = 0; line < index; line++) offset += doc.child(line).content.size + 1;
	// Between lines, which only a select-all reaches, is the next line's
	// start, or the end of the text after the last line.
	if ($pos.depth === 0) return index === doc.childCount ? offset - 1 : offset;
	return offset + $pos.parentOffset;
};

/** Makes a list key's edit as one undo step, and places the selection it gives. */
const applyListEdit = (view: EditorView, text: string, edit: ListEdit) => {
	const { state } = view;
	const tr = state.tr;
	const positionOf = positionsIn(text);
	for (const each of [...edit.replacements].reverse())
		tr.replace(positionOf(each.from), positionOf(each.to), sliceOf(state.schema, each.text));
	const placed = positionsIn(applyReplacements(text, edit.replacements));
	tr.setSelection(
		TextSelection.create(tr.doc, placed(edit.selection.anchor), placed(edit.selection.head))
	);
	view.dispatch(closeHistory(tr).scrollIntoView());
};

/**
 * Puts a visual's block in at `pos`, or over the selection, as one undo
 * step, with the caret on the line after it. A place inside another
 * visual's lines moves to their end, so a block never splits one.
 */
const putVisual = (
	view: EditorView,
	visual: Visual,
	pos: number | "selection"
): Placed | { readonly refused: string } => {
	const { selection } = view.state;
	const target = pos === "selection" ? selection.from : pos;
	const around = visualLinesAt(view.state, target);
	// A selection over a whole visual goes, and the new one takes its place.
	const covered =
		pos === "selection" &&
		!!around &&
		selection.from <= around.from &&
		selection.to >= around.to;
	const tr = view.state.tr;
	if (pos === "selection") tr.deleteSelection();
	const where = tr.mapping.map(around && !covered ? around.to : target);
	const text = markedTextOf(tr.doc);
	const placing = placeVisual(text, offsetOf(tr.doc, where), visual);
	if ("refused" in placing) return placing;
	const { placed, body } = placing;
	const from = positionsIn(text)(placed.at);
	tr.replace(from, from, sliceOf(view.state.schema, placed.text));
	// Past a block that ends on its closing line is a blank line already.
	const end = placed.at + placed.text.length + (placed.text.endsWith("\n") ? 0 : 1);
	tr.setSelection(TextSelection.create(tr.doc, positionsIn(body)(Math.min(end, body.length))));
	view.dispatch(closeHistory(tr).scrollIntoView());
	return placed;
};

/** The note's text and the selection as offsets into it. */
const textAndSelection = (view: EditorView) => {
	const { doc, selection } = view.state;
	return {
		text: markedTextOf(doc),
		offsets: { anchor: offsetOf(doc, selection.anchor), head: offsetOf(doc, selection.head) },
	};
};

/**
 * Option-Up and Option-Down move the lines the selection touches, and with
 * Shift copy them, each as one undo step. In a list or a code block too.
 */
const lineKeys = (view: EditorView, event: KeyboardEvent): boolean => {
	if (!view.editable || event.isComposing || event.keyCode === 229) return false;
	if (!event.altKey || event.metaKey || event.ctrlKey) return false;
	if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return false;
	event.preventDefault();
	const { text, offsets } = textAndSelection(view);
	const direction = event.key === "ArrowUp" ? -1 : 1;
	const edit = (event.shiftKey ? copyLines : moveLines)(text, offsets, direction);
	if (edit.replacements.length > 0) applyListEdit(view, text, edit);
	return true;
};

/** Enter, Shift-Enter, Tab and Shift-Tab in a note: lists carry on, and move in and out. */
const listKeys = (view: EditorView, event: KeyboardEvent): boolean => {
	if (!view.editable || event.isComposing || event.keyCode === 229) return false;
	if (event.altKey || event.metaKey || event.ctrlKey) return false;
	if (event.key !== "Enter" && event.key !== "Tab") return false;
	if (event.key === "Enter" && event.shiftKey) {
		event.preventDefault();
		return splitBlock(view.state, view.dispatch);
	}
	// A line in a code block that looks like an item is only code.
	if (enterAddsLine(view.state)) return false;
	const { text, offsets } = textAndSelection(view);
	const edit =
		event.key === "Enter"
			? continueList(text, offsets)
			: shiftList(text, offsets, event.shiftKey);
	if (!edit) return false;
	event.preventDefault();
	if (edit.replacements.length > 0) applyListEdit(view, text, edit);
	return true;
};

type Flash = { readonly id: number; readonly from: number; readonly to: number };

const INCOMING = new PluginKey<DecorationSet>("noteIncoming");

/** Lights the text a change from elsewhere put in, until it fades. */
const IncomingFlash = Extension.create({
	name: "noteIncoming",
	addProseMirrorPlugins: () => [
		new Plugin<DecorationSet>({
			key: INCOMING,
			state: {
				init: () => DecorationSet.empty,
				apply: (tr, set) => {
					let next = set.map(tr.mapping, tr.doc);
					const meta = tr.getMeta(INCOMING) as
						{ add?: readonly Flash[]; clear?: number } | undefined;
					if (meta?.clear !== undefined)
						next = next.remove(
							next.find(undefined, undefined, (spec) => spec.id === meta.clear)
						);
					if (meta?.add?.length)
						next = next.add(
							tr.doc,
							meta.add.map(({ id, from, to }) =>
								Decoration.inline(from, to, { class: "note-incoming" }, { id })
							)
						);
					return next;
				},
			},
			props: { decorations: (state) => INCOMING.getState(state) ?? null },
		}),
	],
});

/** A text handed out, and how many changes the document had then. */
type Known = { readonly print: string; readonly at: number };

export const NoteEditor = forwardRef<NoteEditorHandle, NoteEditorProps>(
	function NoteEditor(props, ref) {
		const { t } = useI18n();

		const latest = useRef(props);
		useLayoutEffect(() => {
			latest.current = props;
		});
		const visuals = useNoteVisuals();
		const native = useComposerNative();
		const [nativeSpelling] = useState(() => native.hasNativeSpelling());
		const typing = useEditorTyping(native);
		const [extensions] = useState(() => [
			Document,
			Paragraph,
			Text,
			UndoRedo,
			MarkdownLook.configure({ ...NOTE_LOOK, visuals: visuals.host }),
			CodeColours,
			IncomingFlash,
			...(nativeSpelling
				? [Spelling.configure({ native, on: typing.mode === "native" })]
				: []),
		]);

		// Every change's steps, from the `dropped`th on, and the texts handed
		// out with how far into them each was. A change from elsewhere made
		// from one of those texts moves through the steps after it.
		const history = useRef({
			maps: [] as StepMap[],
			dropped: 0,
			known: [{ print: fingerprint(props.initial), at: 0 }] as Known[],
			applying: false,
			flashes: 0,
		});

		const remember = (text: string) => {
			const kept = history.current;
			kept.known.push({ print: fingerprint(text), at: kept.dropped + kept.maps.length });
			if (kept.known.length <= KNOWN_TEXTS) return;
			kept.known.splice(0, kept.known.length - KNOWN_TEXTS);
			const oldest = kept.known[0]!.at;
			kept.maps.splice(0, oldest - kept.dropped);
			kept.dropped = oldest;
		};

		const editor = useEditor({
			extensions,
			editable: !props.readOnly,
			editorProps: {
				attributes: {
					class: "note-editor",
					role: "textbox",
					"aria-multiline": "true",
					"aria-label": t("Note"),
					...typing.attributes,
				},
				// A copy is the text as written, one line break between lines.
				clipboardTextSerializer: (slice) =>
					slice.content.textBetween(0, slice.content.size, "\n"),
				handleKeyDown: (view, event) => lineKeys(view, event) || listKeys(view, event),
				// Pasted text goes in as plain lines, blank ones and all.
				handlePaste: (view, event) => {
					const visual = event.clipboardData && transferredVisual(event.clipboardData);
					if (visual) {
						event.preventDefault();
						const put =
							"refused" in visual
								? visual
								: putVisual(view, visual.visual, "selection");
						if ("refused" in put) latest.current.onRefuse?.(put.refused);
						return true;
					}
					const pasted = event.clipboardData?.getData("text/plain") ?? "";
					if (pasted === "") return false;
					event.preventDefault();
					const text = pasted.replace(/\r\n?/g, "\n");
					view.dispatch(
						view.state.tr
							.replaceSelection(sliceOf(view.state.schema, text))
							.scrollIntoView()
					);
					return true;
				},
				// A visual dragged in from a card goes in where it's dropped.
				handleDrop: (view, event) => {
					const visual = event.dataTransfer && transferredVisual(event.dataTransfer);
					if (!visual) return false;
					event.preventDefault();
					if (!view.editable) return true;
					const at = view.posAtCoords({ left: event.clientX, top: event.clientY });
					const put =
						"refused" in visual
							? visual
							: putVisual(view, visual.visual, at?.pos ?? view.state.selection.head);
					if ("refused" in put) latest.current.onRefuse?.(put.refused);
					else view.focus();
					return true;
				},
			},
			onTransaction: ({ transaction }) => {
				if (transaction.docChanged) history.current.maps.push(...transaction.mapping.maps);
			},
			onUpdate: ({ editor: changed }) => {
				const text = markedTextOf(changed.state.doc);
				remember(text);
				if (!history.current.applying) latest.current.onChange(text);
			},
			onBlur: () => latest.current.onBlur?.(),
		});

		// The text goes in once the editor is up, since a drawing in it is a
		// React root of its own, which can't start during this render. It
		// isn't a change: nothing hears of it and undo skips it.
		const [initial] = useState(() => props.initial);
		useLayoutEffect(() => {
			if (initial === "" || editor.isDestroyed) return;
			const { state } = editor;
			const tr = state.tr.replaceWith(
				0,
				state.doc.content.size,
				paragraphsOf(state.schema, initial)
			);
			tr.setSelection(TextSelection.atStart(tr.doc));
			const kept = history.current;
			kept.applying = true;
			try {
				editor.view.dispatch(tr.setMeta("addToHistory", false));
			} finally {
				kept.applying = false;
			}
			kept.maps = [];
			kept.dropped = 0;
			kept.known = [{ print: fingerprint(initial), at: 0 }];
		}, [editor, initial]);

		const readOnly = props.readOnly ?? false;
		useEffect(() => {
			if (!editor.isDestroyed && editor.isEditable === readOnly)
				editor.setEditable(!readOnly, false);
		}, [editor, readOnly]);

		useSpellingSwitch(editor, typing.mode);

		useImperativeHandle(
			ref,
			() => ({
				text: () => markedTextOf(editor.state.doc),
				focus: () => void editor.commands.focus(),
				insertVisual: (visual) =>
					editor.isDestroyed
						? { refused: translate("The note closed.") }
						: putVisual(editor.view, visual, editor.state.selection.head),
				removePlaced: (placed) => {
					if (editor.isDestroyed) return false;
					const { state } = editor;
					const text = markedTextOf(state.doc);
					const where = placedAt(text, placed);
					if (where < 0) return false;
					const positionOf = positionsIn(text);
					editor.view.dispatch(
						closeHistory(
							state.tr.delete(
								positionOf(where),
								positionOf(where + placed.text.length)
							)
						)
					);
					return true;
				},
				applyIncoming: (text, against) => {
					if (editor.isDestroyed) return;
					const kept = history.current;
					const print = fingerprint(against);
					const known = kept.known.findLast((each) => each.print === print);
					const current = markedTextOf(editor.state.doc);
					// A text this editor never handed out can only be read
					// against what it shows now.
					const base = known ? against : current;
					if (!known) console.warn("A note's change came from a text it doesn't know");
					const since = known ? known.at - kept.dropped : kept.maps.length;
					const mapping = new Mapping(kept.maps.slice(since));
					const positionOf = positionsIn(base);
					const { state } = editor;
					const tr = state.tr;
					const placed: { from: number; to: number; text: string }[] = [];
					for (const each of replacementsBetween(base, text)) {
						const from = mapping.map(positionOf(each.from), 1);
						const to = Math.max(from, mapping.map(positionOf(each.to), -1));
						placed.push({ from, to, text: each.text });
					}
					// From the end, so each change leaves the ones before it in place.
					const lit: { step: number; from: number; to: number }[] = [];
					for (const each of placed.reverse()) {
						const step = tr.steps.length;
						tr.replace(each.from, each.to, sliceOf(state.schema, each.text));
						if (each.text !== "") lit.push({ step, from: each.from, to: each.to });
					}
					if (!tr.docChanged) {
						remember(current);
						return;
					}
					const id = ++kept.flashes;
					const add = lit
						.map(({ step, from, to }) => {
							const after = tr.mapping.slice(step);
							return { id, from: after.map(from, -1), to: after.map(to, 1) };
						})
						.filter((flash) => flash.from < flash.to);
					tr.setMeta("addToHistory", false).setMeta(INCOMING, { add });
					kept.applying = true;
					try {
						editor.view.dispatch(tr);
					} finally {
						kept.applying = false;
					}
					setTimeout(() => {
						if (!editor.isDestroyed)
							editor.view.dispatch(editor.state.tr.setMeta(INCOMING, { clear: id }));
					}, INCOMING_FLASH_MS);
					// What was typed here since stays, so the text differs from
					// what came in, and whoever keeps it hears of that.
					const merged = markedTextOf(editor.state.doc);
					if (merged !== text) latest.current.onChange(merged);
				},
			}),
			[editor]
		);

		return (
			<>
				<EditorContent editor={editor} className="note-content" />
				<NoteVisualBlocks visuals={visuals} readOnly={readOnly} />
			</>
		);
	}
);
