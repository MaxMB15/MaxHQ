// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Going from prompt to prompt in a conversation. The rail beside it, the
// arrows in its Jump to latest pill and Option+Up/Down all step through the
// same marks with these, so they always agree on where a step goes. A
// conversation opens on its newest page, so the prompts before it may not
// be loaded yet: a step back past the first loaded one loads older pages
// until it finds one, and nothing counts the prompts until all are in.

import type { ItemId, TimelineItem } from "../bindings";
import { t } from "../i18n";
import type { TimelineState } from "../timeline/apply";
import { userText } from "../timeline/export";

/** A prompt you sent, by its ID and its place among the rows. */
export type PromptRow = { readonly id: ItemId; readonly row: number };

/**
 * The prompts among the rows. Rows leave out queued and cancelled
 * messages, so these are the ones the agent got.
 */
export const promptRows = (
	rows: readonly ItemId[],
	items: ReadonlyMap<ItemId, TimelineItem>
): PromptRow[] =>
	rows.flatMap((id, row) => (items.get(id)?.kind === "user_message" ? [{ id, row }] : []));

/** The last prompt among the rows before `ordinal`, or null when none is loaded. */
export const promptBefore = (
	state: Pick<TimelineState, "rows" | "items">,
	ordinal: number
): ItemId | null => {
	let found: ItemId | null = null;
	for (const id of state.rows) {
		const item = state.items.get(id);
		if (!item || item.ordinal >= ordinal) break;
		if (item.kind === "user_message") found = id;
	}
	return found;
};

/**
 * How many prompts there are, or null while older pages may hold more,
 * since a count of the loaded ones would claim a total it doesn't know.
 */
export const promptTotal = (loaded: number, older: boolean): number | null =>
	older ? null : loaded;

/** A prompt you sent, by where its row starts from the list's top, in pixels. */
export type PromptMark = { readonly top: number };

/** Where a step lands. */
export type PromptStep =
	| { readonly kind: "prompt"; readonly index: number }
	/** Past the last prompt: the latest message. */
	| { readonly kind: "latest" }
	/** Before the first prompt loaded: a prompt in the pages still to load. */
	| { readonly kind: "older" };

// A prompt this close to the top of the view counts as at it.
const AT_TOP = 8;

/** What a step needs to know besides the marks. */
export type StepPlace = {
	/** How far the list is scrolled. */
	readonly top: number;
	/** Whether pages before the first loaded one are still to come. */
	readonly older: boolean;
	/** Whether the list shows its end, where it can't scroll further down. */
	readonly atEnd: boolean;
};

/** The prompt before the view's top, or one in the older pages when there are some. */
export const earlierPrompt = (
	marks: readonly PromptMark[],
	place: StepPlace
): PromptStep | null => {
	for (let i = marks.length - 1; i >= 0; i--)
		if (marks[i]!.top < place.top - AT_TOP) return { kind: "prompt", index: i };
	return place.older ? { kind: "older" } : null;
};

/**
 * The prompt after the view's top, or the latest message past the last one.
 * Nothing at the end, since every prompt there already shows.
 */
export const laterPrompt = (marks: readonly PromptMark[], place: StepPlace): PromptStep | null => {
	if (place.atEnd) return null;
	const index = marks.findIndex((mark) => mark.top > place.top + AT_TOP);
	return index >= 0 ? { kind: "prompt", index } : { kind: "latest" };
};

/** A step along the prompts: -1 for the earlier one, 1 for the later one. */
export const stepPrompt = (
	marks: readonly PromptMark[],
	place: StepPlace,
	delta: -1 | 1
): PromptStep | null => (delta < 0 ? earlierPrompt(marks, place) : laterPrompt(marks, place));

/** The prompt nearest the top of the view: the last one at or above it, else the first. */
export const promptAt = (marks: readonly PromptMark[], top: number): number => {
	let at = 0;
	for (let i = 0; i < marks.length; i++) if (marks[i]!.top <= top + AT_TOP) at = i;
	return at;
};

/** Where the view stands among the prompts. */
export type PromptPlace = {
	/** The prompt nearest the top of the view. */
	readonly at: number;
	readonly earlier: PromptStep | null;
	readonly later: PromptStep | null;
};

export const promptPlace = (marks: readonly PromptMark[], place: StepPlace): PromptPlace => ({
	at: promptAt(marks, place.top),
	earlier: earlierPrompt(marks, place),
	later: laterPrompt(marks, place),
});

const stepKey = (step: PromptStep | null): string =>
	step === null ? "-" : step.kind === "prompt" ? String(step.index) : step.kind;

/** A key that changes when the place does, to tell when to draw again. */
export const placeKey = (place: PromptPlace): string =>
	`${place.at} ${stepKey(place.earlier)} ${stepKey(place.later)}`;

/** The first line of a prompt, short enough for a preview. */
export const promptLine = (text: string, max = 80): string => {
	const line = text.trim().split("\n", 1)[0]!.trim();
	return line.length > max ? `${line.slice(0, max - 1).trimEnd()}…` : line;
};

// A message's text changes only with a new version of the item.
const lines = new WeakMap<TimelineItem, string>();

/** The first line of a prompt's item, worked out once for each version of it. */
export const itemLine = (item: TimelineItem | undefined): string => {
	if (item?.kind !== "user_message") return "";
	let line = lines.get(item);
	if (line === undefined) {
		line = promptLine(userText(item));
		lines.set(item, line);
	}
	return line;
};

/**
 * What a prompt previews, such as "2 of 5 · Fix the header", or only its
 * line while the total isn't known.
 */
export const promptPreview = (index: number, total: number | null, line: string): string =>
	total === null
		? line
		: t("{{index}} of {{total}} · {{line}}", { index: index + 1, total, line });

/** What a step's button previews, or null when there's nothing that way. */
export const stepPreview = (
	step: PromptStep | null,
	total: number | null,
	lineOf: (index: number) => string
): string | null => {
	if (step === null) return null;
	switch (step.kind) {
		case "prompt":
			return promptPreview(step.index, total, lineOf(step.index));
		case "latest":
			return t("Latest message");
		case "older":
			return t("Earlier prompt");
	}
};
