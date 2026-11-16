// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A sent message's text for the phone's Markdown view, with each pill the
// phone can open as a link the view draws as a chip: an agent, a note, a
// pull request or issue, a visual, or a picture. A tap on one names its
// piece, so the bubble opens it. A pill the phone can't open, such as a
// quote or a file other than a picture, shows as its plain text, and so
// does any pill inside code, which shows as typed.

import type { AgentId } from "@maxhq/shared/bindings";
import { codeSpans, inCode } from "@maxhq/shared/pills/draft";
import {
	fileLabel,
	gitItemLabel,
	middleTruncate,
	noteLabel,
	visualLabel,
} from "@maxhq/shared/pills/label";
import { pieceText, piecesSource, type SentFacts, type SentPiece } from "@maxhq/shared/pills/sent";

/** What a tap on a sent pill opens. */
export type PillTap = "agent" | "note" | "git" | "visual" | "image";

const SCHEME = "maxhq-pill:";

/** The pattern of every pill's link, which the Markdown view styles as a chip. */
export const PILL_LINK = `^${SCHEME}`;

/** The pattern of a visual's link, which gets the visual's own color. */
export const VISUAL_LINK = `^${SCHEME}visual/`;

/** What a tap on the piece opens, or null when the phone can't open it. */
export const tapOf = (piece: SentPiece, factsOf: SentFacts): PillTap | null => {
	if (piece.type === "text") return null;
	if (piece.type === "git") return "git";
	const { ref } = piece;
	switch (ref.type) {
		case "agent":
		case "note":
		case "visual":
			return ref.type;
		case "attachment":
			return factsOf(ref.id)?.kind === "image" ? "image" : null;
		case "quote":
			return null;
	}
};

/** The piece a pill's link names, or null for any other link. */
export const pillIndex = (url: string): number | null => {
	const match = /^maxhq-pill:[a-z]+\/(\d+)$/.exec(url);
	return match ? Number(match[1]) : null;
};

// A backslash shows any ASCII punctuation as itself.
const literal = (text: string): string => text.replace(/[!-/:-@[-`{-~]/g, "\\$&");

const labelOf = (
	piece: Exclude<SentPiece, { type: "text" }>,
	factsOf: SentFacts,
	numbers: ReadonlyMap<string, number>,
	titleOf: (agent: AgentId) => string | undefined
): string => {
	if (piece.type === "git") return gitItemLabel(piece.ref);
	const { ref } = piece;
	switch (ref.type) {
		case "agent":
			return titleOf(ref.agent) ?? ref.title;
		case "note":
			return noteLabel(ref.title);
		case "visual":
			return visualLabel(ref.title);
		case "attachment":
			return fileLabel(
				factsOf(ref.id) ?? { name: "file", kind: "missing" },
				numbers.get(ref.id)
			);
		case "quote":
			return ref.text;
	}
};

/**
 * The message as Markdown for the phone's view. `titleOf` gives an agent's
 * title now, for one renamed since.
 */
export const sentMarkdown = (
	pieces: readonly SentPiece[],
	factsOf: SentFacts,
	numbers: ReadonlyMap<string, number>,
	titleOf: (agent: AgentId) => string | undefined = () => undefined
): string => {
	const code = codeSpans(piecesSource(pieces));
	let at = 0;
	let out = "";
	pieces.forEach((piece, index) => {
		if (piece.type === "text") {
			out += piece.text;
			at += piece.text.length;
			return;
		}
		const plain = pieceText(piece, factsOf, numbers);
		const tap = tapOf(piece, factsOf);
		if (inCode(code, at)) out += plain;
		else if (tap === null) out += literal(plain);
		else {
			const label = middleTruncate(
				labelOf(piece, factsOf, numbers, titleOf).replace(/\s+/g, " ").trim()
			);
			out += `[${literal(label)}](${SCHEME}${tap}/${index})`;
		}
		// Each piece's mark in `piecesSource` takes two UTF-16 units.
		at += 2;
	});
	return out;
};
