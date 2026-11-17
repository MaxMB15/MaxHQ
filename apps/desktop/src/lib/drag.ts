// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

/**
 * Follows the pointer that went down until it lets go, the drag is cancelled
 * or the window loses focus, with `cursor` over the whole page and no text
 * selected meanwhile. It hears the moves before the page does, so a pane
 * that stops the events can't lose the drag.
 */
export const followPointer = (
	down: { readonly pointerId: number },
	cursor: string,
	move: (event: PointerEvent) => void,
	end: () => void
): void => {
	const id = down.pointerId;
	const body = document.body.style;
	const before = { cursor: body.cursor, userSelect: body.userSelect };
	body.cursor = cursor;
	body.userSelect = "none";
	const onMove = (event: PointerEvent) => {
		if (event.pointerId === id) move(event);
	};
	const onUp = (event: PointerEvent) => {
		if (event.pointerId === id) stop();
	};
	const stop = () => {
		window.removeEventListener("pointermove", onMove, { capture: true });
		window.removeEventListener("pointerup", onUp, { capture: true });
		window.removeEventListener("pointercancel", onUp, { capture: true });
		window.removeEventListener("blur", stop);
		body.cursor = before.cursor;
		body.userSelect = before.userSelect;
		end();
	};
	window.addEventListener("pointermove", onMove, { capture: true });
	window.addEventListener("pointerup", onUp, { capture: true });
	window.addEventListener("pointercancel", onUp, { capture: true });
	window.addEventListener("blur", stop);
};
