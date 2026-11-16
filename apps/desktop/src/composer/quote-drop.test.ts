// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { render } from "@testing-library/react";
import type { QuoteRef } from "@maxhq/shared/pills/draft";
import { QUOTE_MIME } from "@maxhq/shared/pills/quote";
import { VISUAL_MIME, writeVisualTransfer } from "@maxhq/shared/visuals/transfer";
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { FILE_DRAG_TYPE } from "@/agent-view/file-drag";
import type { DropHandlers } from "@/native/drop";

import { browserNative, ComposerNativeProvider } from "./native";
import {
	dropQuote,
	dropVisual,
	quoteOfDrag,
	registerQuoteDrop,
	useQuoteDrop,
	visualOfDrag,
} from "./quote-drop";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";

const quote: QuoteRef = {
	type: "quote",
	agent: "a1",
	agent_title: "Fix the build",
	item: "i1",
	text: "the part you want",
	position: 0,
	source: "reply",
	from: null,
};

const QUOTE_DRAG = new Map([[QUOTE_MIME, JSON.stringify(quote)]]);

/** A drop the webview hands the text box, carrying `data`. */
const dropOf = (data: Record<string, string>) => {
	const event = new Event("drop", { cancelable: true }) as DragEvent;
	Object.defineProperties(event, {
		dataTransfer: { value: { getData: (type: string) => data[type] ?? "" } },
	});
	return event;
};

/** A native side that keeps the handlers each element registers. */
const registering = () => {
	const targets = new Map<Element, DropHandlers>();
	const registerDropTarget = vi.fn((element: Element, handlers: DropHandlers) => {
		targets.set(element, handlers);
		return () => targets.delete(element);
	});
	return { targets, registerDropTarget };
};

afterEach(() => {
	quoteInbox.take("a2");
});

describe("quoteOfDrag", () => {
	it("reads the quote a drag from the page carries", () => {
		expect(quoteOfDrag(QUOTE_DRAG)).toEqual(quote);
	});

	it("finds nothing in a drag of something else", () => {
		expect(quoteOfDrag(new Map([[FILE_DRAG_TYPE, "{}"]]))).toBeNull();
		expect(quoteOfDrag(new Map([[QUOTE_MIME, "{}"]]))).toBeNull();
	});
});

describe("dropQuote", () => {
	it("takes a quote dropped on the text box", () => {
		const onQuote = vi.fn();
		const event = dropOf({ [QUOTE_MIME]: JSON.stringify(quote), "text/plain": quote.text });
		expect(dropQuote(event, onQuote)).toBe(true);
		expect(event.defaultPrevented).toBe(true);
		expect(onQuote).toHaveBeenCalledWith(quote);
	});

	it("leaves any other drop to the box", () => {
		const onQuote = vi.fn();
		const event = dropOf({ "text/plain": "the part you want" });
		expect(dropQuote(event, onQuote)).toBe(false);
		expect(event.defaultPrevented).toBe(false);
		expect(onQuote).not.toHaveBeenCalled();
	});
});

describe("registerQuoteDrop", () => {
	it("takes only quotes, and sends each to the agent's composer", () => {
		const { targets, registerDropTarget } = registering();
		const row = document.createElement("button");
		const undo = registerQuoteDrop({ registerDropTarget }, row, "a2");
		const handlers = targets.get(row)!;
		// No files, so a file dragged over the row neither lights it nor lands.
		expect(handlers.onDrop).toBeUndefined();
		expect(handlers.takesPageDrag?.(QUOTE_DRAG, { x: 0, y: 0 })).toBe(true);
		expect(handlers.takesPageDrag?.(new Map([[FILE_DRAG_TYPE, "{}"]]), { x: 0, y: 0 })).toBe(
			false
		);

		handlers.onPageDrop?.(QUOTE_DRAG, { x: 0, y: 0 });
		expect(quoteInbox.take("a2")).toEqual([{ quote, focus: false }]);
		undo();
		expect(targets.has(row)).toBe(false);
	});
});

describe("useQuoteDrop", () => {
	const Row = ({ agent }: { agent: string | null }) =>
		createElement("button", { ref: useQuoteDrop<HTMLButtonElement>(agent) }, "Row");

	it("registers the element while it's mounted, and none for no agent", () => {
		const { targets, registerDropTarget } = registering();
		const native = { ...browserNative, registerDropTarget };
		const tree = (agent: string | null) =>
			createElement(ComposerNativeProvider, { value: native }, createElement(Row, { agent }));
		const { rerender, unmount } = render(tree("a2"));
		expect(targets.size).toBe(1);
		rerender(tree(null));
		expect(targets.size).toBe(0);
		rerender(tree("a2"));
		unmount();
		expect(targets.size).toBe(0);
	});
});

describe("a visual's drop", () => {
	const visual = { html: "<p>Weekly</p>", title: "Weekly usage", from: "Fix the build" };
	const VISUAL_DRAG = new Map([[VISUAL_MIME, writeVisualTransfer(visual)]]);

	it("reads the visual a drag carries, and nothing from a broken one", () => {
		expect(visualOfDrag(VISUAL_DRAG)).toEqual(visual);
		expect(visualOfDrag(new Map([[VISUAL_MIME, "{}"]]))).toBeNull();
		expect(visualOfDrag(QUOTE_DRAG)).toBeNull();
	});

	it("takes a visual dropped on the text box", () => {
		const onVisual = vi.fn();
		const event = dropOf({ [VISUAL_MIME]: writeVisualTransfer(visual), "text/plain": "x" });
		expect(dropVisual(event, onVisual)).toBe(true);
		expect(event.defaultPrevented).toBe(true);
		expect(onVisual).toHaveBeenCalledWith(visual);
		expect(dropVisual(dropOf({ "text/plain": "x" }), onVisual)).toBe(false);
	});

	it("sends a visual dropped on a sidebar agent to its composer", () => {
		const { targets, registerDropTarget } = registering();
		const row = document.createElement("button");
		registerQuoteDrop({ registerDropTarget }, row, "a2");
		const handlers = targets.get(row)!;
		expect(handlers.takesPageDrag?.(VISUAL_DRAG, { x: 0, y: 0 })).toBe(true);
		handlers.onPageDrop?.(VISUAL_DRAG, { x: 0, y: 0 });
		expect(quoteInbox.take("a2")).toEqual([{ visual, focus: false }]);
	});
});
