// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Moving a pill to a new place in the text by dragging it. A press on a
// pill that moves a few pixels turns into a drag, and a thin caret shows
// where in the text the pill would land. Letting go there moves it, as one
// undo step. Escape, or letting go outside the text, leaves it where it
// was. A press that doesn't move selects the pill, as a click does. It's
// the page's own pointer drag, the way the sidebar drags agents, since the
// app's webview hands native drags to Tauri and the page never sees them.
// Holding the pointer near the top or bottom of a box whose text runs
// past it scrolls the text, faster nearer the edge, and the page shows a
// grabbing hand until the drag ends.

import { Extension } from "@tiptap/core";
import { NodeSelection, Plugin, TextSelection } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";

import type { Point } from "@/native/drop";

import { isPill } from "./doc";
import { isOver } from "./file-drop";
import { movePill } from "./placing";

// How far the pointer moves before a press becomes a drag, in pixels.
const THRESHOLD = 4;

const CARET = "pointer-events-none fixed z-[9999] w-[2px] rounded-full bg-primary";

// The class composer.css gives the page while a drag is on.
const DRAGGING = "pill-dragging";

// How near the box's top or bottom edge the pointer scrolls the text, in pixels.
const EDGE = 32;

// How fast the text scrolls with the pointer right at the edge, in pixels a second.
const MAX_SPEED = 900;

// The longest a frame counts for, in milliseconds, so a stalled page doesn't jump.
const LONGEST_FRAME = 50;

// The drag going on in each text box, ended when the box goes away.
const drags = new WeakMap<EditorView, () => void>();

/** The text position at a point on screen, or null when the point is off the text. */
export const positionAt = (view: EditorView, at: Point): number | null => {
	if (!isOver(view.dom, at)) return null;
	try {
		const found = view.posAtCoords({ left: at.x, top: at.y });
		if (!found) return null;
		return TextSelection.near(view.state.doc.resolve(found.pos)).from;
	} catch {
		// jsdom has no layout, and a point outside the window throws in some engines.
		return null;
	}
};

/** The position of the pill a press landed on, or null for a press anywhere else. */
export const pillAt = (view: EditorView, target: EventTarget | null): number | null => {
	if (!(target instanceof Element)) return null;
	const host = target.closest(".pill-host");
	if (host === null || !view.dom.contains(host)) return null;
	try {
		const pos = view.posAtDOM(host, 0);
		return isPill(view.state.doc.nodeAt(pos)) ? pos : null;
	} catch {
		return null;
	}
};

/**
 * How fast the text in `box` scrolls with the pointer at `at`, in pixels a
 * second: up when below 0, down when above it. It's 0 away from the edges,
 * off the box, and when the text can't scroll that way. The speed grows
 * from nothing where the edge zone starts to the most at the edge.
 */
export const scrollSpeed = (box: HTMLElement, at: Point): number => {
	const room = box.scrollHeight - box.clientHeight;
	if (room <= 0) return 0;
	const { left, right, top, bottom } = box.getBoundingClientRect();
	if (at.x < left || at.x > right || at.y < top || at.y > bottom) return 0;
	// A short box keeps a middle that doesn't scroll.
	const zone = Math.min(EDGE, (bottom - top) / 3);
	if (at.y < top + zone && box.scrollTop > 0) return -MAX_SPEED * (1 - (at.y - top) / zone);
	if (at.y > bottom - zone && box.scrollTop < room)
		return MAX_SPEED * (1 - (bottom - at.y) / zone);
	return 0;
};

// Where the drop caret goes for a position, or null when it can't be told.
const caretBox = (view: EditorView, pos: number) => {
	try {
		return view.coordsAtPos(pos);
	} catch {
		return null;
	}
};

// Stops the click that follows letting go, so nothing under the pointer takes it.
const swallowClick = (): void => {
	const swallow = (e: MouseEvent) => {
		e.preventDefault();
		e.stopPropagation();
	};
	window.addEventListener("click", swallow, { capture: true, once: true });
	setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
};

/**
 * Follows a press on the pill at `from`, which turns into a drag once the
 * pointer moves. The caller stops the press's own handling, so it neither
 * selects text nor starts a native drag. If the text changes before the
 * press ends, the pill stays where it is.
 */
export const pressPill = (view: EditorView, from: number, down: MouseEvent): void => {
	let state: "pressed" | "dragging" | "cancelled" = "pressed";
	let target: number | null = null;
	const { doc } = view.state;
	const pill = view.nodeDOM(from);
	const moving = pill instanceof HTMLElement ? pill : null;
	const caret = document.createElement("div");
	caret.className = CARET;
	caret.dataset.pillCaret = "";
	caret.setAttribute("aria-hidden", "true");
	const scroller = view.dom;
	// Where the pointer is, for the scroll to aim the caret from.
	let pointer: Point = { x: down.clientX, y: down.clientY };
	// The frame the scroll waits on, the last one's time, and the part of a pixel left over.
	let frame: number | null = null;
	let then: number | null = null;
	let carry = 0;

	const stopScrolling = () => {
		if (frame !== null) cancelAnimationFrame(frame);
		frame = null;
		then = null;
		carry = 0;
	};
	const hide = () => {
		stopScrolling();
		caret.remove();
		moving?.classList.remove("pill-moving");
		document.body.classList.remove(DRAGGING);
	};
	const aim = (x: number, y: number) => {
		target = positionAt(view, { x, y });
		const box = target === null ? null : caretBox(view, target);
		if (box === null) {
			caret.remove();
			return;
		}
		caret.style.left = `${box.left - 1}px`;
		caret.style.top = `${box.top}px`;
		caret.style.height = `${box.bottom - box.top}px`;
		if (!caret.isConnected) document.body.append(caret);
	};
	// One frame of the scroll: it moves the text, aims the caret again and
	// asks for the next frame, until the pointer leaves the edge zone.
	const scroll = (now: number) => {
		frame = null;
		const speed = state === "dragging" ? scrollSpeed(scroller, pointer) : 0;
		if (speed === 0) return stopScrolling();
		const elapsed = then === null ? 1000 / 60 : Math.min(now - then, LONGEST_FRAME);
		then = now;
		// The box scrolls in whole pixels, so the parts add up until they make one.
		carry += (speed * elapsed) / 1000;
		const step = Math.trunc(carry);
		if (step !== 0) {
			carry -= step;
			const top = scroller.scrollTop;
			scroller.scrollTop = top + step;
			if (scroller.scrollTop !== top) aim(pointer.x, pointer.y);
		}
		frame = requestAnimationFrame(scroll);
	};
	const move = (e: MouseEvent) => {
		if (state === "cancelled") return;
		if (state === "pressed") {
			if (Math.hypot(e.clientX - down.clientX, e.clientY - down.clientY) < THRESHOLD) return;
			state = "dragging";
			document.body.classList.add(DRAGGING);
			moving?.classList.add("pill-moving");
		}
		pointer = { x: e.clientX, y: e.clientY };
		aim(pointer.x, pointer.y);
		if (frame === null && scrollSpeed(scroller, pointer) !== 0)
			frame = requestAnimationFrame(scroll);
	};
	// Selects the pill and takes the focus, as a click on it does.
	const select = () => {
		if (view.state.doc !== doc) return;
		if (!view.hasFocus()) view.focus();
		const selection = NodeSelection.create(view.state.doc, from);
		view.dispatch(view.state.tr.setSelection(selection).setMeta("pointer", true));
	};
	// Moves the pill to where the drag let go, when that's in the text.
	const land = (x: number, y: number) => {
		aim(x, y);
		if (target === null || view.state.doc !== doc) return;
		const tr = movePill(view.state, from, target);
		if (!tr) return;
		view.dispatch(tr.scrollIntoView());
		view.focus();
	};
	const up = (e: MouseEvent) => {
		if (e.button !== 0) return;
		if (state === "pressed") select();
		if (state === "dragging") land(e.clientX, e.clientY);
		if (state !== "pressed") swallowClick();
		end();
	};
	const key = (e: KeyboardEvent) => {
		if (e.key !== "Escape" || state !== "dragging") return;
		e.preventDefault();
		e.stopPropagation();
		state = "cancelled";
		target = null;
		hide();
	};
	const end = () => {
		window.removeEventListener("mousemove", move, { capture: true });
		window.removeEventListener("mouseup", up, { capture: true });
		window.removeEventListener("keydown", key, { capture: true });
		window.removeEventListener("blur", end);
		if (drags.get(view) === end) drags.delete(view);
		hide();
	};

	drags.get(view)?.();
	drags.set(view, end);
	// Before anything else sees them, so a part that stops the events can't lose the drag.
	window.addEventListener("mousemove", move, { capture: true });
	window.addEventListener("mouseup", up, { capture: true });
	window.addEventListener("keydown", key, { capture: true });
	window.addEventListener("blur", end);
};

/** Ends a pill's drag that's still going when its text box goes away. */
export const PillDrag = Extension.create({
	name: "pillDrag",
	addProseMirrorPlugins: () => [
		new Plugin({ view: (view) => ({ destroy: () => drags.get(view)?.() }) }),
	],
});
