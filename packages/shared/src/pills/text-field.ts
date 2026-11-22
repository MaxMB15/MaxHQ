// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A draft in a text field that holds only text, such as the phone's. Each
// pill shows as its token, its label, which the field draws in the pill's
// shade and treats as one piece: typing inside one types after it, and
// deleting any of it deletes it whole. Offsets come in two kinds: the shown
// text's, where a pill is its token, and the marked text's, where a pill is
// one object mark, as the menus and the send path count them.

import { t } from "../i18n";

import {
	type FileFacts,
	fileNumbers,
	gitKey,
	gitNumbers,
	markedText,
	normalize,
	type PlacedPill,
	type Segment,
	withoutMarks,
} from "./draft";
import { fileLabel, middleTruncate, noteLabel, oneLine, quoteLabel, visualLabel } from "./label";
import { spaced } from "./spacing";
import { detectTrigger, type Trigger } from "./trigger";

/** The text a pill shows as in the field. */
export type TokenOf = (pill: PlacedPill) => string;

/** The tokens for a draft's pills: a file's name, an agent's `@title`, a quote's first words. */
export const tokensOf = (
	segments: readonly Segment[],
	factsOf: (key: string) => FileFacts | undefined
): TokenOf => {
	const numbers = fileNumbers(segments, factsOf);
	return (pill) => {
		switch (pill.type) {
			case "attachment": {
				const facts = factsOf(pill.key);
				return middleTruncate(facts ? fileLabel(facts, numbers.get(pill.key)) : t("File"));
			}
			case "visual":
				return middleTruncate(visualLabel(pill.title));
			case "quote":
				return quoteLabel(pill.text);
			case "agent":
				return `@${middleTruncate(pill.title)}`;
			case "note":
				return middleTruncate(noteLabel(pill.title));
			case "output":
				return middleTruncate(oneLine(pill.command));
			case "session":
				return t("Agent shell");
		}
	};
};

/** One pill in the shown text. */
export type FieldPiece = {
	/** Its token's span in the shown text. */
	readonly from: number;
	readonly to: number;
	/** Its mark's offset in the marked text. */
	readonly at: number;
	readonly pill: PlacedPill;
};

/** A draft as the field shows it. */
export type Field = {
	readonly text: string;
	readonly pieces: readonly FieldPiece[];
};

/** The shown text, with where each pill's token sits. */
export const fieldOf = (segments: readonly Segment[], tokenOf: TokenOf): Field => {
	let text = "";
	let marked = 0;
	const pieces: FieldPiece[] = [];
	for (const segment of segments) {
		if (segment.type === "text") {
			text += segment.text;
			marked += segment.text.length;
			continue;
		}
		// A token is never empty, so deleting it shows in the text.
		const token = withoutMarks(tokenOf(segment.pill)) || "?";
		pieces.push({
			from: text.length,
			to: text.length + token.length,
			at: marked,
			pill: segment.pill,
		});
		text += token;
		marked += 1;
	}
	return { text, pieces };
};

/** The piece whose token holds `offset` strictly inside it, or null. */
export const pieceInside = (field: Field, offset: number): FieldPiece | null =>
	field.pieces.find((piece) => piece.from < offset && offset < piece.to) ?? null;

/**
 * A shown offset as a marked one. Inside a token it goes to the token's
 * start, or with `after` to its end.
 */
export const markedOf = (field: Field, shown: number, after = false): number => {
	let shift = 0;
	for (const piece of field.pieces) {
		if (shown <= piece.from) break;
		if (shown < piece.to) return piece.at + (after ? 1 : 0);
		shift += piece.to - piece.from - 1;
	}
	return shown - shift;
};

/** A marked offset as a shown one. */
export const shownOf = (field: Field, marked: number): number => {
	let shift = 0;
	for (const piece of field.pieces) {
		if (marked <= piece.at) break;
		shift += piece.to - piece.from - 1;
	}
	return marked + shift;
};

const lengthOf = (segment: Segment) => (segment.type === "text" ? segment.text.length : 1);

/** The segments before and after a marked offset. */
export const splitMarked = (
	segments: readonly Segment[],
	offset: number
): [Segment[], Segment[]] => {
	const before: Segment[] = [];
	const after: Segment[] = [];
	let at = 0;
	for (const segment of segments) {
		const length = lengthOf(segment);
		if (at + length <= offset) before.push(segment);
		else if (at >= offset) after.push(segment);
		else if (segment.type === "text") {
			before.push({ type: "text", text: segment.text.slice(0, offset - at) });
			after.push({ type: "text", text: segment.text.slice(offset - at) });
		}
		at += length;
	}
	return [before, after];
};

/** A draft after an edit, with the caret's marked offset. */
export type FieldEdit = { readonly segments: Segment[]; readonly caret: number };

/**
 * The draft with the marked span `from` to `to` replaced by `inserted`,
 * with a space where a pill would run into a word, and after any insert
 * with `always`, such as a picked `#123`. The caret goes after it.
 */
export const replaceMarked = (
	segments: readonly Segment[],
	from: number,
	to: number,
	inserted: readonly Segment[],
	always = false
): FieldEdit => {
	const [head] = splitMarked(segments, from);
	const [, tail] = splitMarked(segments, to);
	const before = markedText(head);
	const after = markedText(tail);
	const fitted = spaced(
		normalize(inserted),
		before.slice(before.lastIndexOf("\n") + 1),
		after.split("\n")[0] ?? "",
		always
	);
	const caret = from + markedText(normalize(fitted.segments)).length + (fitted.skipSpace ? 1 : 0);
	return { segments: normalize([...head, ...fitted.segments, ...tail]), caret };
};

/** The shown text's span an edit replaced, and what it put there. */
type Change = { readonly from: number; readonly to: number; readonly typed: string };

/** A selection in the shown text. */
export type FieldSelection = { readonly start: number; readonly end: number };

// The change an edit at `selection` made, or null when no edit there turns
// `old` into `next`: typing over it, or deleting back from its caret.
const changeAt = (old: string, next: string, selection: FieldSelection): Change | null => {
	const { start, end } = selection;
	const typedLength = next.length - (old.length - (end - start));
	if (typedLength >= 0) {
		const typed = next.slice(start, start + typedLength);
		if (old.slice(0, start) + typed + old.slice(end) === next)
			return { from: start, to: end, typed };
	}
	const gone = old.length - next.length;
	if (start === end && gone > 0 && end - gone >= 0)
		if (old.slice(0, end - gone) + old.slice(end) === next)
			return { from: end - gone, to: end, typed: "" };
	return null;
};

/**
 * The first of `candidates` an edit could have started from to turn `old`
 * into `next`, or null when none could. A field that reports its new
 * selection before its new text, as iOS does, leaves the one before the
 * edit second to last.
 */
export const selectionBefore = (
	old: string,
	next: string,
	candidates: readonly (FieldSelection | null)[]
): FieldSelection | null =>
	candidates.find((each) => each !== null && changeAt(old, next, each) !== null) ?? null;

const changeOf = (old: string, next: string, selection: FieldSelection | null): Change => {
	// The selection before the edit tells where it happened, which the text
	// alone can't when a letter repeats.
	const at = selection && changeAt(old, next, selection);
	if (at) return at;
	const most = Math.min(old.length, next.length);
	let head = 0;
	while (head < most && old[head] === next[head]) head += 1;
	let tail = 0;
	while (tail < most - head && old[old.length - 1 - tail] === next[next.length - 1 - tail])
		tail += 1;
	return { from: head, to: old.length - tail, typed: next.slice(head, next.length - tail) };
};

/** A draft after the field's text changed, with the caret's shown offset. */
export type FieldChange = {
	readonly segments: Segment[];
	/** The caret's offset in the shown text. */
	readonly caret: number;
	/** Whether the text holds what the field shows, or the field must show it anew. */
	readonly same: boolean;
};

/**
 * The draft after the field's text became `next`, given the selection the
 * field had before. Text typed inside a token goes after it, and a
 * deletion that touches a token takes the whole pill.
 */
export const editField = (
	segments: readonly Segment[],
	tokenOf: TokenOf,
	next: string,
	selection: FieldSelection | null = null
): FieldChange => {
	const field = fieldOf(segments, tokenOf);
	if (next === field.text)
		return { segments: [...segments], caret: selection?.end ?? next.length, same: true };
	let { from, to, typed } = changeOf(field.text, next, selection);
	typed = withoutMarks(typed);
	if (from === to) {
		const inside = pieceInside(field, from);
		if (inside) {
			from = inside.to;
			to = inside.to;
		}
	} else {
		for (const piece of field.pieces) {
			if (piece.from < to && piece.to > from) {
				from = Math.min(from, piece.from);
				to = Math.max(to, piece.to);
			}
		}
	}
	const markedFrom = markedOf(field, from);
	const markedTo = markedOf(field, to, true);
	const [head] = splitMarked(segments, markedFrom);
	const [, tail] = splitMarked(segments, markedTo);
	const edited = normalize([...head, { type: "text", text: typed }, ...tail]);
	const caret = shownOf(fieldOf(edited, tokenOf), markedFrom + typed.length);
	return { segments: edited, caret, same: fieldOf(edited, tokenOf).text === next };
};

/** The menu the word at the caret opens, or null, with the caret as a shown offset. */
export const fieldTrigger = (
	segments: readonly Segment[],
	tokenOf: TokenOf,
	caret: number
): Trigger | null => {
	const field = fieldOf(segments, tokenOf);
	if (pieceInside(field, caret)) return null;
	return detectTrigger(markedText(segments), markedOf(field, caret));
};

/** A run of the shown text, as the field draws it. */
export type FieldSpan =
	| { readonly kind: "text"; readonly text: string }
	| { readonly kind: "pill"; readonly text: string; readonly pill: PlacedPill }
	| { readonly kind: "git"; readonly text: string; readonly key: string };

/**
 * The shown text in runs: plain text, each pill's token, and each `#123`
 * that `shownGit` says shows as a pull request or issue.
 */
export const fieldSpans = (
	segments: readonly Segment[],
	tokenOf: TokenOf,
	shownGit: (key: string) => boolean = () => false
): FieldSpan[] => {
	const field = fieldOf(segments, tokenOf);
	type Range = { from: number; to: number; make: (text: string) => FieldSpan };
	const ranges: Range[] = [
		...field.pieces.map((piece): Range => ({
			from: piece.from,
			to: piece.to,
			make: (text) => ({ kind: "pill", text, pill: piece.pill }),
		})),
		...gitNumbers(markedText(segments)).flatMap((span): Range[] => {
			const key = gitKey(span);
			if (!shownGit(key)) return [];
			return [
				{
					from: shownOf(field, span.from),
					to: shownOf(field, span.to),
					make: (text) => ({ kind: "git", text, key }),
				},
			];
		}),
	].sort((a, b) => a.from - b.from);
	const out: FieldSpan[] = [];
	let at = 0;
	for (const range of ranges) {
		if (range.from < at) continue;
		if (range.from > at) out.push({ kind: "text", text: field.text.slice(at, range.from) });
		out.push(range.make(field.text.slice(range.from, range.to)));
		at = range.to;
	}
	if (at < field.text.length) out.push({ kind: "text", text: field.text.slice(at) });
	return out;
};
