// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
	isSelectionRectVisible,
	observeSelectionActions,
	resolveSelectionActionPosition,
	SELECTION_MULTI_CLICK_INTERVAL_MS,
} from "./selection-actions";

const bounds = { left: 100, top: 50, width: 600, height: 400 };
const viewport = { width: 1000, height: 800 };

describe("resolveSelectionActionPosition", () => {
	it("goes where the pointer let go", () => {
		expect(
			resolveSelectionActionPosition({
				bounds,
				selectionRect: { right: 300, bottom: 120 },
				pointer: { x: 250, y: 200 },
				viewport,
			})
		).toEqual({ x: 250, y: 200 });
	});

	it("goes below the selection's end without a pointer", () => {
		expect(
			resolveSelectionActionPosition({
				bounds,
				selectionRect: { right: 300, bottom: 120 },
				pointer: null,
				viewport,
			})
		).toEqual({ x: 300, y: 124 });
	});

	it("stays inside the bounds and the window", () => {
		expect(
			resolveSelectionActionPosition({
				bounds,
				selectionRect: null,
				pointer: { x: 20, y: 2000 },
				viewport,
			})
		).toEqual({ x: 100, y: 450 });
		expect(
			resolveSelectionActionPosition({
				bounds: { left: 0, top: 0, width: 2000, height: 2000 },
				selectionRect: { right: 1500, bottom: 1500 },
				pointer: null,
				viewport,
			})
		).toEqual({ x: 992, y: 792 });
	});
});

describe("isSelectionRectVisible", () => {
	const rect = { left: 10, top: 10, right: 50, bottom: 30 };

	it("shows inside every clip", () => {
		expect(isSelectionRectVisible(rect, [{ left: 0, top: 0, right: 100, bottom: 100 }])).toBe(
			true
		);
	});

	it("hides when any clip cuts it off", () => {
		expect(
			isSelectionRectVisible(rect, [
				{ left: 0, top: 0, right: 100, bottom: 100 },
				{ left: 0, top: 40, right: 100, bottom: 100 },
			])
		).toBe(false);
		// Only touching an edge shows nothing.
		expect(isSelectionRectVisible(rect, [{ left: 50, top: 0, right: 100, bottom: 100 }])).toBe(
			false
		);
	});
});

describe("observeSelectionActions", () => {
	let element: HTMLElement;
	let action: HTMLButtonElement;

	beforeEach(() => {
		vi.useFakeTimers({
			toFake: ["setTimeout", "clearTimeout", "requestAnimationFrame", "cancelAnimationFrame"],
		});
		element = document.createElement("div");
		element.innerHTML = "<p>Some text to pick</p>";
		action = document.createElement("button");
		document.body.append(element, action);
	});

	afterEach(() => {
		vi.useRealTimers();
		document.body.innerHTML = "";
	});

	const watch = () => {
		const onSelection = vi.fn();
		const onDismiss = vi.fn();
		const actions = observeSelectionActions({
			element,
			getActionElement: () => action,
			onSelection,
			onDismiss,
		});
		return { actions, onSelection, onDismiss };
	};

	const pick = (detail = 1) => {
		const text = element.querySelector("p")!;
		text.dispatchEvent(
			new PointerEvent("pointerdown", { bubbles: true, isPrimary: true, button: 0 })
		);
		window.dispatchEvent(
			new MouseEvent("mouseup", { button: 0, clientX: 40, clientY: 12, detail })
		);
	};

	it("offers the actions where a pick let go, once it settles", () => {
		const { actions, onSelection } = watch();
		pick();
		expect(actions.pending).toBe(true);
		vi.advanceTimersByTime(20);
		expect(onSelection).toHaveBeenCalledWith({ x: 40, y: 12 });
		actions.dispose();
	});

	it("waits out a double click", () => {
		const { actions, onSelection } = watch();
		pick(2);
		vi.advanceTimersByTime(SELECTION_MULTI_CLICK_INTERVAL_MS - 1);
		expect(onSelection).not.toHaveBeenCalled();
		vi.advanceTimersByTime(20);
		expect(onSelection).toHaveBeenCalledTimes(1);
		actions.dispose();
	});

	it("puts the actions away when the selection is dragged", () => {
		const { actions, onSelection, onDismiss } = watch();
		pick();
		element.querySelector("p")!.dispatchEvent(new Event("dragstart", { bubbles: true }));
		expect(onDismiss).toHaveBeenCalledWith("interaction");
		vi.advanceTimersByTime(20);
		expect(onSelection).not.toHaveBeenCalled();
		actions.dispose();
	});

	it("puts the actions away on Escape", () => {
		const { actions, onDismiss } = watch();
		pick();
		document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
		expect(onDismiss).toHaveBeenCalledWith("cancel");
		expect(actions.pending).toBe(false);
		actions.dispose();
	});
});
