// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";

import { scrollSpeed } from "./pill-drag";
import {
	editorOf,
	findTextbox,
	installBrowserFakes,
	renderComposer,
	textIn,
	type,
} from "./testing";

beforeAll(installBrowserFakes);

afterEach(() => {
	vi.useRealTimers();
	vi.restoreAllMocks();
});

// The text box, 400 wide and 100 tall at the window's top left.
const BOX = new DOMRect(0, 0, 400, 100);

/** "Read", main.rs and "now", with the pill at 6 and the end at 11. */
const readNow = async () => {
	const composer = renderComposer();
	const textbox = await findTextbox();
	await type(textbox, "Read");
	act(() => composer.drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
	await waitFor(() => expect(screen.getByText(/^1 attachment,/)).toBeInTheDocument());
	await type(textbox, "now");
	expect(textIn(textbox)).toBe(`Read ${MARK} now`);
	return textbox;
};

/**
 * Lays the text box out, which jsdom can't, with text that runs `room`
 * pixels past its bottom. A point over it lands at 11, and the caret
 * there moves up as the text scrolls.
 */
const layOut = (textbox: HTMLElement, room: number, from = 0) => {
	let top = from;
	vi.spyOn(textbox, "getBoundingClientRect").mockReturnValue(BOX);
	Object.defineProperties(textbox, {
		clientHeight: { configurable: true, get: () => BOX.height },
		scrollHeight: { configurable: true, get: () => BOX.height + room },
		scrollTop: {
			configurable: true,
			get: () => top,
			set: (value: number) => {
				top = Math.max(0, Math.min(room, value));
			},
		},
	});
	// The editor scrolls the caret into view, and jsdom can't scroll the window.
	vi.spyOn(window, "scrollBy").mockImplementation(() => {});
	const { view } = editorOf(textbox);
	view.posAtCoords = () => ({ pos: 11, inside: -1 });
	view.coordsAtPos = () => ({ left: 120, right: 120, top: 10 - top, bottom: 28 - top });
	// The frames come from a clock the test runs, once the composer is up.
	vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame"] });
};

const press = (textbox: HTMLElement) =>
	fireEvent.mouseDown(textbox.querySelector(".pill")!, { button: 0, clientX: 50, clientY: 10 });
const moveTo = (x: number, y: number) => fireEvent.mouseMove(window, { clientX: x, clientY: y });
const letGo = (x: number, y: number) =>
	fireEvent.mouseUp(window, { button: 0, clientX: x, clientY: y });
const dropCaret = () => document.querySelector<HTMLElement>("[data-pill-caret]");
const frames = (count: number) => vi.advanceTimersByTime(16 * count);
const dragging = () => document.body.classList.contains("pill-dragging");

describe("the scroll a dragged pill makes", () => {
	it("is faster nearer the edge, and nothing away from it", async () => {
		const textbox = await readNow();
		layOut(textbox, 300, 100);

		const at = (y: number) => scrollSpeed(textbox, { x: 150, y });
		expect(at(50)).toBe(0);
		expect(at(90)).toBeGreaterThan(0);
		expect(at(99)).toBeGreaterThan(at(90));
		expect(at(10)).toBeLessThan(0);
		expect(at(1)).toBeLessThan(at(10));
		// Off the box, it doesn't scroll.
		expect(scrollSpeed(textbox, { x: 500, y: 99 })).toBe(0);
		expect(scrollSpeed(textbox, { x: 150, y: 120 })).toBe(0);
	});

	it("scrolls the text down near the bottom, with the caret, until it lets go", async () => {
		const textbox = await readNow();
		layOut(textbox, 300);

		press(textbox);
		moveTo(150, 50);
		expect(vi.getTimerCount()).toBe(0);
		moveTo(150, 95);
		expect(vi.getTimerCount()).toBe(1);
		frames(10);
		const scrolled = textbox.scrollTop;
		expect(scrolled).toBeGreaterThan(0);
		// The caret follows the text it would land in.
		expect(dropCaret()).toHaveStyle({ top: `${10 - scrolled}px` });

		// Nearer the edge, the same time scrolls further.
		moveTo(150, 99);
		frames(10);
		expect(textbox.scrollTop - scrolled).toBeGreaterThan(scrolled);

		letGo(150, 99);
		expect(vi.getTimerCount()).toBe(0);
		const after = textbox.scrollTop;
		frames(10);
		expect(textbox.scrollTop).toBe(after);
	});

	it("scrolls up near the top, and stops once the pointer leaves the edge", async () => {
		const textbox = await readNow();
		layOut(textbox, 300, 200);

		press(textbox);
		moveTo(150, 3);
		frames(10);
		expect(textbox.scrollTop).toBeLessThan(200);

		moveTo(150, 50);
		frames(1);
		expect(vi.getTimerCount()).toBe(0);
		const after = textbox.scrollTop;
		frames(10);
		expect(textbox.scrollTop).toBe(after);
		letGo(150, 50);
	});

	it("stops at the end of the text", async () => {
		const textbox = await readNow();
		layOut(textbox, 20);

		press(textbox);
		moveTo(150, 99);
		frames(60);
		expect(textbox.scrollTop).toBe(20);
		expect(vi.getTimerCount()).toBe(0);
		letGo(150, 99);
	});

	it("stops on Escape", async () => {
		const textbox = await readNow();
		layOut(textbox, 300);
		const before = editorOf(textbox).state.doc;

		press(textbox);
		moveTo(150, 99);
		frames(5);
		fireEvent.keyDown(window, { key: "Escape" });
		expect(vi.getTimerCount()).toBe(0);
		expect(dropCaret()).toBeNull();
		const after = textbox.scrollTop;
		moveTo(150, 98);
		frames(10);
		expect(textbox.scrollTop).toBe(after);
		expect(vi.getTimerCount()).toBe(0);
		letGo(150, 98);
		expect(editorOf(textbox).state.doc).toBe(before);
	});

	it("doesn't start when the text fits the box", async () => {
		const textbox = await readNow();
		layOut(textbox, 0);

		press(textbox);
		moveTo(150, 99);
		expect(vi.getTimerCount()).toBe(0);
		frames(10);
		expect(textbox.scrollTop).toBe(0);
		letGo(150, 99);
	});

	it("leaves nothing running when the text box goes away mid drag", async () => {
		const textbox = await readNow();
		layOut(textbox, 300);
		const editor = editorOf(textbox);
		const added = vi.spyOn(window, "addEventListener");
		const removed = vi.spyOn(window, "removeEventListener");

		press(textbox);
		const listeners = added.mock.calls.filter(([kind]) =>
			["mousemove", "mouseup", "keydown", "blur"].includes(kind)
		);
		expect(listeners).toHaveLength(4);
		moveTo(150, 99);
		frames(2);
		expect(vi.getTimerCount()).toBe(1);

		cleanup();
		await waitFor(() => expect(editor.isDestroyed).toBe(true));
		expect(vi.getTimerCount()).toBe(0);
		expect(dropCaret()).toBeNull();
		expect(dragging()).toBe(false);
		for (const [kind, listener] of listeners)
			expect(removed).toHaveBeenCalledWith(
				kind,
				listener,
				...(kind === "blur" ? [] : [{ capture: true }])
			);
	});
});

describe("the cursor over a pill", () => {
	it("grabs the whole page only while a press is a drag", async () => {
		const textbox = await readNow();
		layOut(textbox, 0);

		press(textbox);
		expect(dragging()).toBe(false);
		moveTo(52, 11);
		expect(dragging()).toBe(false);
		moveTo(150, 12);
		expect(dragging()).toBe(true);
		// It stays on off the text too, where a drop leaves the pill where it was.
		moveTo(150, 300);
		expect(dragging()).toBe(true);
		letGo(150, 300);
		expect(dragging()).toBe(false);
	});

	it("lets go of the page on Escape", async () => {
		const textbox = await readNow();
		layOut(textbox, 0);

		press(textbox);
		moveTo(150, 12);
		expect(dragging()).toBe(true);
		fireEvent.keyDown(window, { key: "Escape" });
		expect(dragging()).toBe(false);
		letGo(150, 12);
		expect(dragging()).toBe(false);
	});

	it("doesn't grab the page for a click", async () => {
		const textbox = await readNow();
		layOut(textbox, 0);

		press(textbox);
		letGo(50, 10);
		expect(dragging()).toBe(false);
		expect(editorOf(textbox).state.selection.from).toBe(6);
	});
});
