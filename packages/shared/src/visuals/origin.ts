// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where a visual came from: the agent's reply or visual item that holds it.
// Tell the agent quotes a visual's errors from there into that agent's
// composer, so a visual elsewhere, such as in a file or a note, has no one
// to tell. The desktop and the phone hand it to their cards the same way.

import { createContext, use } from "react";

import type { AgentId, TimelineItem } from "../bindings";
import type { QuoteRef } from "../pills/draft";
import { makeQuote } from "../pills/quote";
import type { VisualError } from "./reports";

export type VisualOrigin = {
	readonly agent: { readonly id: AgentId; readonly title: string };
	readonly item: TimelineItem;
};

export const VisualOriginContext = createContext<VisualOrigin | null>(null);

/** The reply around a visual, when it's in one whose agent takes messages. */
export const useVisualOrigin = (): VisualOrigin | null => use(VisualOriginContext);

/** What Tell the agent puts in the quote: the errors, each on its own line. */
export const errorsQuote = (
	title: string,
	errors: readonly VisualError[],
	count: number
): string => {
	const lines = errors.map(({ message, line }) =>
		line === null ? message : `Line ${line}: ${message}`
	);
	const more = count - errors.length;
	return [
		`The visual${title ? ` "${title}"` : ""} showed ${count === 1 ? "an error" : `${count} errors`}:`,
		...lines,
		...(more > 0 ? [`and ${more} more`] : []),
	].join("\n");
};

/** The quote Tell the agent sends: the errors, placed where the visual is in its reply. */
export const tellQuote = (
	origin: VisualOrigin,
	html: string,
	title: string,
	errors: readonly VisualError[],
	count: number
): QuoteRef => {
	const at = origin.item.kind === "assistant_message" ? origin.item.text.indexOf(html) : -1;
	return makeQuote(origin.agent, origin.item, {
		text: errorsQuote(title, errors, count),
		position: Math.max(0, at),
	});
};
