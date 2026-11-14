// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import { type DraggedFile, FILE_DRAG_TYPE } from "@/agent-view/file-drag";

import { dropTimelineFile, isOver, timelineFileOf } from "./file-drop";

const FILE: DraggedFile = {
	device: "dev-devbox",
	agent: "a2",
	path: "/attachments/a2/notes.pdf",
	name: "notes.pdf",
	kind: "pdf",
	mime: "application/pdf",
	bytes: 3200,
};

/** A drop the webview hands the text box, carrying `data`. */
const dropOf = (data: Record<string, string>) => {
	const event = new Event("drop", { cancelable: true }) as DragEvent;
	Object.defineProperties(event, {
		dataTransfer: { value: { getData: (type: string) => data[type] ?? "" } },
	});
	return event;
};

describe("timelineFileOf", () => {
	it("reads the file a drag from the page carries", () => {
		expect(timelineFileOf(new Map([[FILE_DRAG_TYPE, JSON.stringify(FILE)]]))).toEqual(FILE);
	});

	it("finds nothing in a drag of something else", () => {
		expect(timelineFileOf(new Map([["application/x-maxhq-quote", "{}"]]))).toBeNull();
		expect(timelineFileOf(new Map([[FILE_DRAG_TYPE, "{}"]]))).toBeNull();
	});
});

describe("isOver", () => {
	it("tells whether a point is inside an element's box", () => {
		const element = document.createElement("div");
		vi.spyOn(element, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 400, 600, 120));
		expect(isOver(element, { x: 300, y: 450 })).toBe(true);
		expect(isOver(element, { x: 300, y: 399 })).toBe(false);
		expect(isOver(null, { x: 300, y: 450 })).toBe(false);
	});
});

describe("dropTimelineFile", () => {
	it("takes a timeline file dropped on the text box", () => {
		const onFile = vi.fn();
		const event = dropOf({ [FILE_DRAG_TYPE]: JSON.stringify(FILE), "text/plain": "notes.pdf" });
		expect(dropTimelineFile(event, onFile)).toBe(true);
		expect(event.defaultPrevented).toBe(true);
		expect(onFile).toHaveBeenCalledWith(FILE);
	});

	it("leaves any other drop to the editor", () => {
		const onFile = vi.fn();
		const event = dropOf({ "text/plain": "notes.pdf" });
		expect(dropTimelineFile(event, onFile)).toBe(false);
		expect(event.defaultPrevented).toBe(false);
		expect(onFile).not.toHaveBeenCalled();
	});
});
