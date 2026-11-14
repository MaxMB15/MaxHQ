// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createPageDrags } from "./page-drag";

/** A dragstart from `element`, carrying `data`. */
const dragFrom = (element: Element, data: Record<string, string>) => {
	const event = new Event("dragstart", { bubbles: true });
	Object.defineProperty(event, "dataTransfer", {
		value: { types: Object.keys(data), getData: (type: string) => data[type] ?? "" },
	});
	element.dispatchEvent(event);
};

const end = (element: Element) => element.dispatchEvent(new Event("dragend", { bubbles: true }));

describe("createPageDrags", () => {
	let doc: Document;
	let source: HTMLElement;

	beforeEach(() => {
		vi.useFakeTimers();
		doc = document.implementation.createHTMLDocument();
		source = doc.createElement("button");
		doc.body.append(source);
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	it("notes what a drag of MaxHQ's carries, and nothing else", () => {
		const drags = createPageDrags();
		drags.watch(doc);
		dragFrom(source, { "application/x-maxhq-file": "{}", "text/plain": "shot.png" });
		expect(drags.current()).toEqual(new Map([["application/x-maxhq-file", "{}"]]));

		dragFrom(source, { "text/plain": "some words" });
		expect(drags.current()).toBeNull();
	});

	it("gives the drag to one taker", () => {
		const drags = createPageDrags();
		drags.watch(doc);
		dragFrom(source, { "application/x-maxhq-file": "{}" });
		expect(drags.take()?.get("application/x-maxhq-file")).toBe("{}");
		expect(drags.take()).toBeNull();
		expect(drags.current()).toBeNull();
	});

	it("keeps a drag a moment after it ends, for a drop that comes late", () => {
		const drags = createPageDrags(1000);
		const onEnd = vi.fn();
		drags.onEnd(onEnd);
		drags.watch(doc);
		dragFrom(source, { "application/x-maxhq-file": "{}" });
		end(source);
		expect(onEnd).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(999);
		expect(drags.current()).not.toBeNull();
		vi.advanceTimersByTime(1);
		expect(drags.current()).toBeNull();
	});

	it("hears the end of a drag whose element left the page", () => {
		const drags = createPageDrags(1000);
		drags.watch(doc);
		dragFrom(source, { "application/x-maxhq-file": "{}" });
		source.remove();
		end(source);
		vi.advanceTimersByTime(1000);
		expect(drags.current()).toBeNull();
	});

	it("ends a drag that never heard its end on the next press", () => {
		const drags = createPageDrags(1000);
		drags.watch(doc);
		dragFrom(source, { "application/x-maxhq-file": "{}" });
		source.remove();
		doc.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
		vi.advanceTimersByTime(1000);
		expect(drags.current()).toBeNull();
	});

	it("keeps a new drag when an older one's time runs out", () => {
		const drags = createPageDrags(1000);
		drags.watch(doc);
		dragFrom(source, { "application/x-maxhq-file": "old" });
		end(source);
		vi.advanceTimersByTime(500);
		dragFrom(source, { "application/x-maxhq-file": "new" });
		vi.advanceTimersByTime(1000);
		expect(drags.current()?.get("application/x-maxhq-file")).toBe("new");
	});
});
