// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A quote from what you selected in a reply. The phone's Markdown view gives
// the selected words as they show, not where they sit in the reply's text,
// so the quote starts where those words first show up in the text with
// whitespace runs counted as one space, as the desktop counts, or at the
// start when formatting hides them there. The place only picks which match
// a jump goes to, so the first one is near enough.

import type { QuoteAnchor } from "@maxhq/shared/pills/anchor";

const flat = (text: string): string => text.replace(/\s+/g, " ");

/** The quote of `selected` in a row whose text is `text`, or null when nothing is picked. */
export const quoteOfSelection = (text: string, selected: string): QuoteAnchor | null => {
	const quote = selected.trim();
	if (quote === "") return null;
	return { text: quote, position: Math.max(0, flat(text).indexOf(flat(quote))) };
};
