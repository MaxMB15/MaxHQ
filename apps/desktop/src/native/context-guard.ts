// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Keeps the webview's own context menu, with Reload and Inspect Element,
// out of the app. A right-click on a text field still gets the webview's
// Cut, Copy and Paste, one on selected text gets a native Copy, and one a
// component answered already, such as a MaxHQ menu or the spelling menu,
// is left alone. Anywhere else nothing shows. In a development build,
// holding Option on the right-click lets the webview's menu through, for
// Inspect Element.

import { contextMenu } from "./menu";
import { inApp } from "./platform";

/** Whether a point lands on a range's text. */
export const onText = (range: Range, x: number, y: number): boolean =>
	Array.from(range.getClientRects()).some(
		(line) => x >= line.left && x <= line.right && y >= line.top && y <= line.bottom
	);

/** Whether a right-click lands on something you type into. */
const editable = (target: EventTarget | null): boolean =>
	target instanceof Element &&
	target.closest("input, textarea, select, [contenteditable]:not([contenteditable='false'])") !==
		null;

/** The selected text under a point, if the point lands on it. */
const selectedAt = (x: number, y: number): string | null => {
	const selection = window.getSelection();
	if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
	const text = selection.toString();
	if (text.trim() === "") return null;
	for (let i = 0; i < selection.rangeCount; i++) {
		if (onText(selection.getRangeAt(i), x, y)) return text;
	}
	return null;
};

/** Answers a right-click no component answered. */
export const guardContextMenu = (event: MouseEvent, dev: boolean): void => {
	if (event.defaultPrevented || editable(event.target)) return;
	if (dev && event.altKey) return;
	const text = selectedAt(event.clientX, event.clientY);
	if (
		text !== null &&
		!contextMenu([{ kind: "copy", text }], { x: event.clientX, y: event.clientY })
	)
		return;
	event.preventDefault();
};

/**
 * Starts guarding right-clicks in the window, inside the app only. It
 * listens last, as the event bubbles to the window, so a component's own
 * menu has had its turn. Returns the call that stops it.
 */
export const watchContextMenu = (dev = import.meta.env.DEV): (() => void) => {
	if (!inApp()) return () => {};
	const listener = (event: MouseEvent) => guardContextMenu(event, dev);
	window.addEventListener("contextmenu", listener);
	return () => window.removeEventListener("contextmenu", listener);
};
