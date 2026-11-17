// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The text box itself, TipTap holding plain Markdown as one paragraph per
// line, with a pill for each file, quote and agent placed in it, and the
// Markdown drawn as it will read. Enter belongs to the composer unless it's
// in a code block, Shift+Enter breaks the line, and Up and Down hand over to
// the composer with where the caret is, for the queue and earlier messages.
// While a menu is open, its keys go to the menu first.
// The Enter guard during an IME composition and the caret on the first or
// last line for Up and Down are kept here, and pill-keys.ts
// holds what the pills' keys take from it.
// Drops go in at the caret, and a pill drags to a new place in the text,
// as placing.ts and pill-drag.ts tell.

import type { HistoryCaret, HistoryDirection } from "@maxhq/shared/composer/history";
import type { QuoteRef, Segment } from "@maxhq/shared/pills/draft";
import type { VisualTransfer } from "@maxhq/shared/visuals/transfer";
import Document from "@tiptap/extension-document";
import Paragraph from "@tiptap/extension-paragraph";
import Text from "@tiptap/extension-text";
import { Placeholder, UndoRedo } from "@tiptap/extensions";
import { splitBlock } from "@tiptap/pm/commands";
import { closeHistory } from "@tiptap/pm/history";
import { Fragment, Slice } from "@tiptap/pm/model";
import { EditorState, NodeSelection, TextSelection } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import { type Editor, EditorContent, useEditor } from "@tiptap/react";
import { type Ref, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from "react";

import type { DraggedFile } from "@/agent-view/file-drag";

import {
	fileKeyOfNode,
	isPill,
	markedTextOf,
	offsetOf,
	onlyPillsBefore,
	paragraphsFor,
	posOf,
	segmentsOf,
	segmentsOfFragment,
} from "./doc";
import { CodeColours } from "./code-colours";
import { dropTimelineFile } from "./file-drop";
import { type GitHover, gitHoverAt, GitPills, type GitPillsStore } from "./git-pills";
import type { ComposerNative } from "./native";
import { enterAddsLine, MarkdownLook } from "./markdown";
import { insertLink, PastedLinks, swapLink } from "./pasted-links";
import { pillAt, PillDrag, pressPill } from "./pill-drag";
import { PILL_EXTENSIONS } from "./pill-nodes";
import { deletePill, type Direction, stepOverPill, writeDraftClipboard } from "./pill-keys";
import { CaretPlaced, dropPosition, pillWithSpace, placeSegments } from "./placing";
import { dropQuote, dropVisual } from "./quote-drop";
import { heldFileKeys } from "./retained";
import { Spelling } from "./spelling";
import { useEditorTyping, useSpellingSwitch } from "./typing-prefs";

/** What the composer can do to its text box. */
export type PromptEditorHandle = {
	/** The text, one line per paragraph and each pill as one object mark. */
	readonly text: () => string;
	readonly segments: () => Segment[];
	/** Puts in new text and pills, and moves the caret to the end. */
	readonly setSegments: (segments: readonly Segment[]) => void;
	/** Empties the box and its undo history, once the message is sent. */
	readonly reset: () => void;
	readonly focus: () => void;
	/** Types text at the caret, line breaks and all. */
	readonly insert: (text: string) => void;
	/**
	 * Types a pasted link at the caret and keeps track of it. Gives the
	 * link's ID for `swapLink`, or null when it couldn't be kept track of.
	 */
	readonly insertLink: (text: string) => number | null;
	/**
	 * Puts `text` in place of a link `insertLink` typed, but only while the
	 * link is there as it went in. Null forgets the link. Returns whether
	 * the text went in.
	 */
	readonly swapLink: (id: number, text: string | null) => boolean;
	/**
	 * Puts text and pills in place of the selection, with a space where a
	 * pill would run into a word. It takes the focus unless `focus` is false.
	 */
	readonly insertSegments: (segments: readonly Segment[], focus?: boolean) => void;
	/**
	 * Puts text and pills in the way a drop does: at the caret the box kept,
	 * after any selected text, or at the end when the box hasn't had a caret.
	 * It takes the focus unless `focus` is false.
	 */
	readonly dropSegments: (segments: readonly Segment[], focus?: boolean) => void;
	/**
	 * Replaces the text between two offsets, such as the word a menu pick
	 * replaces. `space` adds a space after it even when it ends in text.
	 */
	readonly replaceRange: (
		from: number,
		to: number,
		segments: readonly Segment[],
		space?: boolean
	) => void;
	/** Takes the pills of these files out, leaving the undo history alone. */
	readonly removeFiles: (keys: ReadonlySet<string>) => void;
};

/** The text box after a change, as the composer sees it. */
export type EditorChange = {
	readonly text: string;
	readonly segments: Segment[];
	/** The caret's offset in the text, or null while text is selected. */
	readonly caret: number | null;
	/** The files undo could bring back, or null when that can't be told. */
	readonly held: ReadonlySet<string> | null;
};

/** A key for an open menu. */
export type MenuKey = "up" | "down" | "pick" | "close";

/** What the text box tells the composer. */
export type PromptEditorEvents = {
	/** Enter, with whether Command or Control was held. */
	readonly onEnter: (withModifier: boolean) => void;
	/** Up or Down with no modifier, and where the caret is. Returns whether it did anything. */
	readonly onHistoryKey: (direction: HistoryDirection, caret: HistoryCaret) => boolean;
	/**
	 * `!` typed on the first line with only pills before it. Returns whether
	 * it took the key, so no `!` goes in.
	 */
	readonly onBang?: () => boolean;
	/** Command or Control and U. */
	readonly onAttach: () => void;
	/** A paste. Returns whether it took the paste, so the text box doesn't. */
	readonly onPaste: (data: DataTransfer) => boolean;
	/** Files dropped by the webview itself, which happens outside the app. */
	readonly onDropFiles: (files: File[]) => void;
	/** A file pulled out of a timeline and dropped by the webview itself. */
	readonly onDropTimelineFile?: (file: DraggedFile) => void;
	/** Each change to the text or the caret. */
	readonly onChange: (change: EditorChange) => void;
	/** A copy or cut with pills in it: their plain form, and the fragment that keeps them. */
	/** The copy's text and fragment, and any more types it goes under, such as a visual's. */
	readonly onCopy: (segments: Segment[]) => {
		plain: string;
		fragment: string | null;
		extra?: Readonly<Record<string, string>>;
	};
	/** A key while a menu may be open. Returns whether the menu took it. */
	readonly onMenuKey: (key: MenuKey) => boolean;
	/** The pointer moved onto a drawn `#123`, or off one. */
	readonly onGitHover: (hover: GitHover | null) => void;
	/** A quote dragged from a timeline and dropped by the webview itself. */
	readonly onDropQuote: (quote: QuoteRef) => void;
	/** A visual dragged from a card and dropped by the webview itself. */
	readonly onDropVisual?: (visual: VisualTransfer) => void;
};

type PromptEditorProps = PromptEditorEvents & {
	readonly ref: Ref<PromptEditorHandle>;
	readonly placeholder: string;
	readonly label: string;
	readonly native: ComposerNative;
	readonly gitPills: GitPillsStore;
	/** ARIA attributes that tie the box to its open menu. */
	readonly menuAttributes?: Readonly<Record<string, string>>;
	/**
	 * What the box holds as it opens, such as the draft kept from when its
	 * pane closed. It's read once, and goes in with the caret at the end,
	 * without the focus or an undo step.
	 */
	readonly initial?: readonly Segment[];
};

/** The text of a document, one line per paragraph and each pill as one object mark. */
export const textOf = markedTextOf;

/**
 * Puts text and pills between two positions, leaving the caret after them,
 * as an undo step of its own.
 */
const place = (
	view: EditorView,
	from: number,
	to: number,
	segments: readonly Segment[],
	always = false
) =>
	view.dispatch(
		closeHistory(placeSegments(view.state.tr, from, to, segments, always)).scrollIntoView()
	);

/** Types text at the selection, with each line break starting a new paragraph. */
const insertText = (view: EditorView, text: string) => {
	const slice = new Slice(
		Fragment.from(paragraphsFor(view.state.schema, [{ type: "text", text }])),
		1,
		1
	);
	view.dispatch(view.state.tr.replaceSelection(slice).scrollIntoView());
};

const hasFiles = (data: DataTransfer | null): data is DataTransfer =>
	data !== null && data.files.length > 0;

const noModifiers = (event: KeyboardEvent | MouseEvent) =>
	!event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey;

const MENU_KEYS: Record<string, MenuKey> = {
	ArrowUp: "up",
	ArrowDown: "down",
	Enter: "pick",
	Tab: "pick",
	Escape: "close",
};

const HISTORY_KEYS: Record<string, HistoryDirection> = {
	ArrowUp: "backward",
	ArrowDown: "forward",
};

/**
 * Where the caret is for Up or Down: before everything, and on the line
 * the key would leave the box from. Nothing counts while text or a pill is
 * selected.
 */
const historyCaret = (view: EditorView, direction: HistoryDirection): HistoryCaret => {
	const { doc, selection } = view.state;
	if (!selection.empty) return { atStart: false, onEdge: false };
	const up = direction === "backward";
	const line = selection.$head.index(0);
	return {
		atStart: offsetOf(doc, selection.head) === 0,
		onEdge: line === (up ? 0 : doc.childCount - 1) && view.endOfTextblock(up ? "up" : "down"),
	};
};

/** Each editor's placeholder, which follows the agent's state. */
const placeholders = new WeakMap<Editor, string>();

const PLACEHOLDER = Placeholder.configure({
	placeholder: ({ editor }) => placeholders.get(editor) ?? "",
});

const changeOf = (state: EditorState): EditorChange => {
	const { selection } = state;
	return {
		text: markedTextOf(state.doc),
		segments: segmentsOf(state.doc),
		caret: selection.empty ? offsetOf(state.doc, selection.head) : null,
		held: heldFileKeys(state),
	};
};

/** The composer's text box. */
export const PromptEditor = (props: PromptEditorProps) => {
	const { ref, placeholder, label, native, gitPills, menuAttributes } = props;
	// TipTap hands the editor new props in an effect, a moment after each
	// render. The handlers call through this instead, which is set as the
	// render commits, so a key pressed in that moment reaches the current
	// callbacks and not the last render's.
	const latest = useRef(props);
	useLayoutEffect(() => {
		latest.current = props;
	});
	// The system checker is in the editor wherever it could run, and the
	// settings switch it on and off without the editor starting over.
	const [nativeSpelling] = useState(() => native.hasNativeSpelling());
	const typing = useEditorTyping(native);
	const [extensions] = useState(() => [
		Document,
		Paragraph,
		Text,
		UndoRedo,
		CaretPlaced,
		PillDrag,
		PLACEHOLDER,
		...PILL_EXTENSIONS,
		GitPills.configure({ store: gitPills }),
		PastedLinks,
		MarkdownLook,
		CodeColours,
		...(nativeSpelling ? [Spelling.configure({ native, on: typing.mode === "native" })] : []),
	]);

	/** Copies the selection with its pills, or leaves a copy without pills to ProseMirror. */
	const copy = (view: EditorView, event: ClipboardEvent, cut: boolean): boolean => {
		const { from, to, empty } = view.state.selection;
		const data = event.clipboardData;
		if (empty || !data) return false;
		const segments = segmentsOfFragment(view.state.doc.slice(from, to).content);
		if (!segments.some((segment) => segment.type === "pill")) return false;
		const { plain, fragment, extra = {} } = latest.current.onCopy(segments);
		event.preventDefault();
		writeDraftClipboard(data, plain, fragment);
		for (const [type, text] of Object.entries(extra)) data.setData(type, text);
		if (cut) view.dispatch(view.state.tr.deleteSelection().scrollIntoView());
		return true;
	};

	const pillKeys = (view: EditorView, event: KeyboardEvent): boolean => {
		const { state } = view;
		const run = (tr: ReturnType<typeof stepOverPill>) => {
			if (!tr) return false;
			event.preventDefault();
			view.dispatch(tr);
			return true;
		};
		const direction: Direction | null =
			event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : null;
		if (direction !== null && noModifiers(event) && !event.isComposing)
			return run(stepOverPill(state, direction));
		if (event.key === "Backspace" && !event.metaKey && !event.ctrlKey)
			return run(deletePill(state, -1, event.altKey));
		if (event.key === "Delete" && !event.metaKey && !event.ctrlKey)
			return run(deletePill(state, 1));
		const { selection } = state;
		if (
			event.key === "Escape" &&
			selection instanceof NodeSelection &&
			isPill(selection.node)
		) {
			event.preventDefault();
			view.dispatch(state.tr.setSelection(TextSelection.create(state.doc, selection.to)));
			return true;
		}
		return false;
	};

	const editor = useEditor({
		extensions,
		editorProps: {
			attributes: {
				class: "composer-editor",
				role: "textbox",
				"aria-multiline": "true",
				"aria-label": label,
				"aria-placeholder": placeholder,
				...typing.attributes,
				...menuAttributes,
			},
			handleKeyDown: (view, event) => {
				if (event.key === "Enter" && (event.isComposing || event.keyCode === 229)) {
					event.stopPropagation();
					return true;
				}
				const { selection, doc } = view.state;
				const first = selection.empty && onlyPillsBefore(doc, selection.head);
				const bang = event.key === "!" && !event.metaKey && !event.ctrlKey && first;
				if (bang && !event.isComposing && latest.current.onBang?.()) {
					event.preventDefault();
					return true;
				}
				const menuKey = MENU_KEYS[event.key];
				if (
					menuKey &&
					(noModifiers(event) || event.key === "Tab") &&
					latest.current.onMenuKey(menuKey)
				) {
					event.preventDefault();
					return true;
				}
				if (pillKeys(view, event)) return true;
				if (event.key === "Enter") {
					event.preventDefault();
					const withModifier = event.metaKey || event.ctrlKey;
					if (
						event.shiftKey ||
						event.altKey ||
						(!withModifier && enterAddsLine(view.state))
					)
						return splitBlock(view.state, view.dispatch);
					latest.current.onEnter(withModifier);
					return true;
				}
				const direction = HISTORY_KEYS[event.key];
				if (direction && noModifiers(event) && !event.isComposing) {
					const caret = historyCaret(view, direction);
					if (!latest.current.onHistoryKey(direction, caret)) return false;
					event.preventDefault();
					return true;
				}
				if (
					(event.metaKey || event.ctrlKey) &&
					!event.shiftKey &&
					event.key.toLowerCase() === "u"
				) {
					event.preventDefault();
					latest.current.onAttach();
					return true;
				}
				return false;
			},
			handlePaste: (view, event) => {
				const data = event.clipboardData;
				if (!data) return false;
				if (latest.current.onPaste(data)) {
					event.preventDefault();
					return true;
				}
				const text = data.getData("text/plain");
				if (text === "") return false;
				event.preventDefault();
				insertText(view, text);
				return true;
			},
			handleDrop: (_view, event) => {
				const { onDropQuote, onDropVisual, onDropTimelineFile } = latest.current;
				if (onDropVisual && dropVisual(event, onDropVisual)) return true;
				if (dropQuote(event, onDropQuote)) return true;
				if (onDropTimelineFile && dropTimelineFile(event, onDropTimelineFile)) return true;
				if (!hasFiles(event.dataTransfer)) return false;
				event.preventDefault();
				latest.current.onDropFiles(Array.from(event.dataTransfer.files));
				return true;
			},
			handleDOMEvents: {
				copy: (view, event) => copy(view, event, false),
				cut: (view, event) => copy(view, event, true),
				// A press on a pill is the pill's own, so ProseMirror neither
				// selects text nor starts a drag of its own from it.
				mousedown: (view, event) => {
					if (event.button !== 0 || !noModifiers(event) || !view.editable) return false;
					const pos = pillAt(view, event.target);
					if (pos === null) return false;
					event.preventDefault();
					pressPill(view, pos, event);
					return true;
				},
				mouseover: (_view, event) => {
					latest.current.onGitHover(gitHoverAt(event.target));
					return false;
				},
				mouseout: (_view, event) => {
					const left = gitHoverAt(event.target);
					if (left && !left.element.contains(event.relatedTarget as Node | null))
						latest.current.onGitHover(null);
					return false;
				},
			},
		},
		onUpdate: ({ editor: changed }) => latest.current.onChange(changeOf(changed.state)),
		onSelectionUpdate: ({ editor: changed }) =>
			latest.current.onChange(changeOf(changed.state)),
	});

	// The draft it opens with goes in once, and the composer hears of it as
	// it would of typing.
	const [initial] = useState(() => props.initial);
	useLayoutEffect(() => {
		if (!initial || initial.length === 0 || editor.isDestroyed) return;
		const { state } = editor;
		const tr = state.tr.replaceWith(
			0,
			state.doc.content.size,
			paragraphsFor(state.schema, initial)
		);
		tr.setSelection(TextSelection.atEnd(tr.doc));
		editor.view.dispatch(tr.setMeta("addToHistory", false));
	}, [editor, initial]);

	useSpellingSwitch(editor, typing.mode);

	// The placeholder follows the agent's state, so redraw it when it changes.
	useEffect(() => {
		placeholders.set(editor, placeholder);
		if (!editor.isDestroyed) editor.view.dispatch(editor.state.tr.setMeta("placeholder", true));
	}, [editor, placeholder]);

	useImperativeHandle(ref, () => {
		return {
			text: () => markedTextOf(editor.state.doc),
			segments: () => segmentsOf(editor.state.doc),
			setSegments: (segments) => {
				const { state } = editor;
				const tr = state.tr.replaceWith(
					0,
					state.doc.content.size,
					paragraphsFor(state.schema, segments)
				);
				tr.setSelection(TextSelection.atEnd(tr.doc));
				editor.view.dispatch(tr.scrollIntoView());
				editor.commands.focus();
			},
			reset: () => {
				const { state } = editor;
				editor.view.updateState(
					EditorState.create({ schema: state.schema, plugins: state.plugins })
				);
				latest.current.onChange(changeOf(editor.state));
			},
			focus: () => {
				editor.commands.focus();
			},
			insert: (text) => {
				insertText(editor.view, text);
				editor.commands.focus();
			},
			insertLink: (text) => {
				const id = insertLink(editor.state, (tr) => editor.view.dispatch(tr), text);
				editor.commands.focus();
				return id;
			},
			swapLink: (id, text) =>
				!editor.isDestroyed &&
				swapLink(editor.state, (tr) => editor.view.dispatch(tr), id, text),
			insertSegments: (segments, focus = true) => {
				const { selection } = editor.state;
				place(editor.view, selection.from, selection.to, segments);
				if (focus) editor.commands.focus();
			},
			dropSegments: (segments, focus = true) => {
				const at = dropPosition(editor.state);
				place(editor.view, at, at, segments);
				if (focus) editor.commands.focus();
			},
			replaceRange: (from, to, segments, space = false) => {
				const { doc } = editor.state;
				place(editor.view, posOf(doc, from), posOf(doc, to), segments, space);
				editor.commands.focus();
			},
			removeFiles: (keys) => {
				const { state } = editor;
				const ranges: { from: number; to: number }[] = [];
				state.doc.descendants((node, pos) => {
					const key = fileKeyOfNode(node);
					if (key === null || !keys.has(key)) return;
					// The space the pill brought goes with it, unless words close up.
					ranges.push(pillWithSpace(state.doc, pos));
				});
				if (ranges.length === 0) return;
				const tr = state.tr;
				for (const range of ranges.reverse()) tr.delete(range.from, range.to);
				editor.view.dispatch(tr.setMeta("addToHistory", false));
			},
		};
	}, [editor]);

	return <EditorContent editor={editor} className="composer-content" />;
};
