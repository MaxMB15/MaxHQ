// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A sent message as its bubble shows it: runs of text, each placed pill
// where its mark sits, and each `#123` that resolved as its pull request or
// issue. The desktop draws these, and the phone can draw the same ones. A
// copy of some of it carries its pills, so a paste in a composer gets them
// back.

import type { AgentId, AttachmentId, AttachmentRef, DeviceId, InlineRef } from "../bindings";

import type { ClipboardFile, ClipboardFragment } from "./clipboard";
import {
	agentMarker,
	excerptIds,
	type FileFacts,
	fileKeys,
	fileMarker,
	fileNumbers,
	fromUserInput,
	type GitItemRef,
	gitKey,
	gitNumbers,
	isPlaced,
	keyedGitItems,
	noteMarker,
	normalize,
	OBJECT_MARK,
	outputMarker,
	type PlacedRef,
	type ReceivedInput,
	type Segment,
	visualMarker,
} from "./draft";

/** One run of text, one placed pill, or a `#123` that resolved. */
export type SentPiece =
	| { readonly type: "text"; readonly text: string }
	| { readonly type: "pill"; readonly ref: PlacedRef }
	/** The `#123` as typed, with the pull request or issue it names. */
	| { readonly type: "git"; readonly text: string; readonly ref: GitItemRef };

/**
 * A sent message in pieces. Each mark takes the next placed ref, and a mark
 * with none drops out. A ref with no mark goes at the end, the way a draft
 * made from the message puts it. Each `#N` outside code whose number has a
 * pull request or issue becomes that one, and so does each `owner/repo#N`
 * with one in that repo. A message with no refs is one run of text.
 */
export const sentPieces = ({ text, refs = [] }: Pick<ReceivedInput, "text" | "refs">) => {
	const placed = refs.filter(isPlaced);
	const keyed = keyedGitItems(
		text,
		refs.filter((ref): ref is GitItemRef => ref.type === "git_item")
	);
	const pieces: SentPiece[] = [];
	const pushText = (run: string) => {
		if (run === "") return;
		const last = pieces.at(-1);
		if (last?.type === "text")
			pieces[pieces.length - 1] = { type: "text", text: last.text + run };
		else pieces.push({ type: "text", text: run });
	};
	let next = 0;
	const cut = (chunk: string) =>
		chunk.split(OBJECT_MARK).forEach((run, index) => {
			const ref = index > 0 ? placed[next++] : undefined;
			if (ref) pieces.push({ type: "pill", ref });
			pushText(run);
		});
	let at = 0;
	for (const span of keyed.size > 0 ? gitNumbers(text) : []) {
		const ref = keyed.get(gitKey(span));
		if (!ref) continue;
		cut(text.slice(at, span.from));
		pieces.push({ type: "git", text: text.slice(span.from, span.to), ref });
		at = span.to;
	}
	cut(text.slice(at));
	for (const ref of placed.slice(next)) {
		const last = pieces.at(-1);
		if (last && !(last.type === "text" && /\s$/.test(last.text))) pushText(" ");
		pieces.push({ type: "pill", ref });
	}
	return pieces;
};

// A code point past anything a font or a keyboard gives, so a piece's mark
// can't be read as the text around it.
const FIRST_PIECE_MARK = 0x100000;

/** The mark that stands for the piece at `index` in `piecesSource`. */
export const pieceMark = (index: number): string => String.fromCodePoint(FIRST_PIECE_MARK + index);

/** The index of the piece a mark stands for, or null when it's some other text. */
export const pieceIndexOf = (mark: string): number | null => {
	const point = mark.codePointAt(0);
	return point !== undefined && point >= FIRST_PIECE_MARK && mark.length === 2
		? point - FIRST_PIECE_MARK
		: null;
};

/**
 * A sent message's text for a Markdown parser, each pill and `#123` one mark
 * from `pieceMark`, since a label read as Markdown would change the text
 * around it.
 */
export const piecesSource = (pieces: readonly SentPiece[]): string =>
	pieces.map((piece, index) => (piece.type === "text" ? piece.text : pieceMark(index))).join("");

/** What a sent message knows about each of its files. */
export type SentFacts = (id: AttachmentId) => FileFacts | undefined;

/**
 * Each placed image's and PDF's number, by its attachment, counted per kind
 * in text order the way the agent's markers count them. A file with no
 * pill has none.
 */
export const sentNumbers = (
	pieces: readonly SentPiece[],
	factsOf: SentFacts
): Map<AttachmentId, number> =>
	fileNumbers(
		pieces.flatMap((piece): Segment[] =>
			piece.type === "pill" && piece.ref.type === "attachment"
				? [{ type: "pill", pill: { type: "attachment", key: piece.ref.id } }]
				: []
		),
		factsOf
	);

/**
 * A piece as plain text, for another app's paste: a file as its marker,
 * such as `[Image 1: shot.png]`, a quote in quote marks, an agent as
 * `[Agent: title]`, a note as `[Note: title]`, a visual as
 * `[Visual: title]`, a command's output as `[Output: command, exit 1]` and
 * a `#123` as typed.
 */
export const pieceText = (
	piece: SentPiece,
	factsOf: SentFacts,
	numbers: ReadonlyMap<AttachmentId, number>
): string => {
	if (piece.type !== "pill") return piece.text;
	const { ref } = piece;
	switch (ref.type) {
		case "attachment":
			return fileMarker(
				factsOf(ref.id) ?? { name: "file", kind: "file" },
				numbers.get(ref.id)
			);
		case "quote":
			return `"${ref.text}"`;
		case "agent":
			return agentMarker(ref.title);
		case "note":
			return noteMarker(ref.title);
		case "visual":
			return visualMarker(ref.title);
		case "output":
			return outputMarker(ref);
	}
};

/** A sent message as plain text, each piece the way `pieceText` gives it. */
export const sentText = (pieces: readonly SentPiece[], factsOf: SentFacts): string => {
	const numbers = sentNumbers(pieces, factsOf);
	return pieces.map((piece) => pieceText(piece, factsOf, numbers)).join("");
};

/**
 * A sent message as its bubble needs it: its pieces, its files by ID, each
 * placed file's number, and the files that show. An agent's excerpt goes to
 * the agent without showing as a file.
 */
export const sentMessage = (
	item: Pick<ReceivedInput, "text" | "refs"> & { readonly attachments: readonly AttachmentRef[] }
) => {
	const files = new Map(item.attachments.map((file) => [file.id, file]));
	const factsOf = (id: AttachmentId) => files.get(id);
	const pieces = sentPieces(item);
	const excerpts = excerptIds(item.refs);
	return {
		pieces,
		files,
		factsOf,
		numbers: sentNumbers(pieces, factsOf),
		shown: item.attachments.filter((file) => !excerpts.has(file.id)),
	};
};

/** The agent a message went to, and the device it's on, if the view knows. */
export type SentPlace = {
	readonly agent: AgentId;
	readonly device: DeviceId | null;
};

/** The origin of a copy from a sent message, which no composer shares. */
export const SENT_ORIGIN = "sent";

const sentKey = (id: AttachmentId) => `sent-${id}`;

// A sent file named where the agent has it, or nowhere when that isn't known.
const sentFile = (
	key: string,
	file: AttachmentRef | undefined,
	place: SentPlace
): ClipboardFile => ({
	key,
	name: file?.name ?? "file",
	kind: file?.kind ?? "missing",
	source: null,
	ref: null,
	device: null,
	found:
		file && place.device !== null
			? { agent: place.agent, device: place.device, path: file.path }
			: null,
});

/**
 * Pieces of a sent message as a clipboard fragment, so a paste in a
 * composer gets their pills back. A staged file goes to an agent once, so
 * each file names where the agent has it instead, and a paste stages it
 * again from there. A file the message no longer has, or one whose device
 * isn't known, names nowhere and pastes as its marker. A `#123` stays
 * text, with the pull request or issue it names. An agent's excerpt or a
 * note's copy was taken for that send, so the next send takes a new one.
 */
export const sentFragment = (
	pieces: readonly SentPiece[],
	fileOf: (id: AttachmentId) => AttachmentRef | undefined,
	place: SentPlace
): ClipboardFragment => {
	const segments: Segment[] = [];
	const files = new Map<string, ClipboardFile>();
	const gitItems = new Map<string, GitItemRef>();
	for (const piece of pieces) {
		if (piece.type !== "pill") {
			segments.push({ type: "text", text: piece.text });
			if (piece.type === "git" && !gitItems.has(gitKey(piece.ref)))
				gitItems.set(gitKey(piece.ref), piece.ref);
			continue;
		}
		const { ref } = piece;
		switch (ref.type) {
			case "attachment": {
				const key = sentKey(ref.id);
				files.set(key, sentFile(key, fileOf(ref.id), place));
				segments.push({ type: "pill", pill: { type: "attachment", key } });
				break;
			}
			case "quote":
				segments.push({ type: "pill", pill: ref });
				break;
			case "agent":
				segments.push({ type: "pill", pill: { ...ref, excerpt: null } });
				break;
			case "note":
				segments.push({ type: "pill", pill: { ...ref, copy: null } });
				break;
			case "visual": {
				const key = sentKey(ref.attachment);
				files.set(key, sentFile(key, fileOf(ref.attachment), place));
				const { title, from = null } = ref;
				segments.push({ type: "pill", pill: { type: "visual", key, title, from } });
				break;
			}
		}
	}
	return {
		version: 1,
		origin: SENT_ORIGIN,
		segments: normalize(segments),
		files: [...files.values()],
		gitItems: [...gitItems.values()],
	};
};

/** A sent message, as a timeline item has it. */
export type SentInput = {
	readonly text: string;
	readonly attachments: readonly AttachmentRef[];
	readonly refs?: readonly InlineRef[];
};

/**
 * A whole sent message as a fragment, such as when Up brings it back to a
 * composer. Its pills are the ones `sentFragment` gives, and each file it
 * sent with no place in the text goes at the end, the way a draft made
 * from the message puts it.
 */
export const messageFragment = (message: SentInput, place: SentPlace): ClipboardFragment => {
	const files = new Map(message.attachments.map((file) => [sentKey(file.id), file]));
	const { text, refs } = message;
	const attachments = message.attachments.map((file) => file.id);
	const draft = fromUserInput({ text, attachments, refs }, sentKey);
	return {
		version: 1,
		origin: SENT_ORIGIN,
		segments: draft.segments,
		files: fileKeys(draft.segments).map((key) => sentFile(key, files.get(key), place)),
		gitItems: draft.gitItems,
	};
};
