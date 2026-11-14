// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const menu = vi.hoisted(() => ({ contextMenu: vi.fn(() => true) }));
vi.mock("./menu", () => menu);

import { guardContextMenu } from "./context-guard";

const rightClick = (target: Element, init: MouseEventInit = {}) => {
	const event = new MouseEvent("contextmenu", {
		bubbles: true,
		cancelable: true,
		clientX: 5,
		clientY: 5,
		...init,
	});
	target.dispatchEvent(event);
	return event;
};

describe("the right-click guard", () => {
	const listener = (event: MouseEvent) => guardContextMenu(event, false);

	beforeEach(() => {
		document.body.innerHTML =
			'<p id="text">Some words</p><input id="field" /><div contenteditable="true"><span id="note">x</span></div>';
		window.addEventListener("contextmenu", listener);
	});

	afterEach(() => {
		window.removeEventListener("contextmenu", listener);
		window.getSelection()?.removeAllRanges();
		menu.contextMenu.mockClear();
		vi.restoreAllMocks();
	});

	it("keeps the webview's menu off plain parts of the page", () => {
		expect(rightClick(document.getElementById("text")!).defaultPrevented).toBe(true);
		expect(menu.contextMenu).not.toHaveBeenCalled();
	});

	it("lets text fields keep their own menu", () => {
		expect(rightClick(document.getElementById("field")!).defaultPrevented).toBe(false);
		expect(rightClick(document.getElementById("note")!).defaultPrevented).toBe(false);
	});

	it("leaves a menu a component showed alone", () => {
		const text = document.getElementById("text")!;
		text.addEventListener("contextmenu", (event) => event.preventDefault(), { once: true });
		rightClick(text);
		expect(menu.contextMenu).not.toHaveBeenCalled();
	});

	it("offers Copy on selected text under the click", () => {
		const text = document.getElementById("text")!;
		const range = document.createRange();
		range.selectNodeContents(text);
		window.getSelection()!.addRange(range);
		vi.spyOn(Range.prototype, "getClientRects").mockReturnValue([
			{ left: 0, right: 10, top: 0, bottom: 10 },
		] as unknown as DOMRectList);
		expect(rightClick(text).defaultPrevented).toBe(true);
		expect(menu.contextMenu).toHaveBeenCalledWith([{ kind: "copy", text: "Some words" }], {
			x: 5,
			y: 5,
		});
	});

	it("lets the webview's menu through with Option held in a development build", () => {
		const event = new MouseEvent("contextmenu", { cancelable: true, altKey: true });
		guardContextMenu(event, true);
		expect(event.defaultPrevented).toBe(false);
	});
});
