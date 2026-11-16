// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Quotes and visuals dragged from a timeline onto a composer or onto an
// agent in the sidebar. In the app, Tauri's drop brings them, as a drag
// from the page the drop router hands over. In a browser, the composer's
// text box takes a drop on itself.

import type { AgentId } from "@maxhq/shared/bindings";
import type { QuoteRef } from "@maxhq/shared/pills/draft";
import { parseQuote, QUOTE_MIME } from "@maxhq/shared/pills/quote";
import {
	readVisualTransfer,
	VISUAL_MIME,
	type VisualTransfer,
} from "@maxhq/shared/visuals/transfer";
import { useEffect, useState } from "react";

import { claimPageDrop, type PageDrag } from "@/native/drop";

import { type ComposerNative, useComposerNative } from "./native";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";

/** The quote a drag from the page carries, or null. */
export const quoteOfDrag = (drag: PageDrag): QuoteRef | null => parseQuote(drag.get(QUOTE_MIME));

/** The visual a drag from the page carries, or null. */
export const visualOfDrag = (drag: PageDrag): VisualTransfer | null => {
	const carried = drag.get(VISUAL_MIME);
	return carried ? readVisualTransfer(carried) : null;
};

/**
 * Takes a quote the webview dropped on the text box itself, and hands it to
 * `onQuote`. Returns whether the drop was one, so nothing else takes it.
 */
export const dropQuote = (event: DragEvent, onQuote: (quote: QuoteRef) => void): boolean => {
	const quote = parseQuote(event.dataTransfer?.getData(QUOTE_MIME));
	if (quote === null) return false;
	event.preventDefault();
	if (claimPageDrop()) onQuote(quote);
	return true;
};

/** Takes a visual the webview dropped on the text box itself, as `dropQuote` does a quote. */
export const dropVisual = (
	event: DragEvent,
	onVisual: (visual: VisualTransfer) => void
): boolean => {
	const carried = event.dataTransfer?.getData(VISUAL_MIME);
	const visual = carried ? readVisualTransfer(carried) : null;
	if (visual === null) return false;
	event.preventDefault();
	if (claimPageDrop()) onVisual(visual);
	return true;
};

/** Sends quotes and visuals dropped on `element` to `agent`'s composer. Returns the undo. */
export const registerQuoteDrop = (
	native: Pick<ComposerNative, "registerDropTarget">,
	element: Element,
	agent: AgentId
): (() => void) =>
	native.registerDropTarget(element, {
		takesPageDrag: (drag) => quoteOfDrag(drag) !== null || visualOfDrag(drag) !== null,
		onPageDrop: (drag) => {
			const visual = visualOfDrag(drag);
			if (visual !== null) {
				quoteInbox.sendVisual(agent, visual, false);
				return;
			}
			const quote = quoteOfDrag(drag);
			if (quote !== null) quoteInbox.send(agent, quote, false);
		},
	});

/**
 * Makes an element, such as an agent's row in the sidebar, take quotes and
 * visuals for `agent`'s composer while it's mounted. Put the ref on the
 * element. Null takes none.
 */
export const useQuoteDrop = <T extends Element>(
	agent: AgentId | null
): ((element: T | null) => void) => {
	const native = useComposerNative();
	const [element, setElement] = useState<T | null>(null);
	useEffect(() => {
		if (element === null || agent === null) return;
		return registerQuoteDrop(native, element, agent);
	}, [agent, element, native]);
	return setElement;
};
