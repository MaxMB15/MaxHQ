// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A row of chips that keeps to the width it's given. Chips that don't fit
// scroll sideways inside it, with no scrollbar, and a soft fade at an end
// says more chips hide past it. Sideways swipes scroll it as they would any
// box. A mouse wheel turned over it scrolls it sideways until it reaches
// that end, and then the list around it scrolls as usual. A wheel that's
// already scrolling the list keeps doing so as the strip passes under the
// pointer, the way a browser keeps one scroller for a whole gesture. A
// press that scrolled the strip before it let go isn't a click. Keyboard
// focus on a chip scrolls all of it into view, clear of the fades.

import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/** The strip's scrolling box, as the browser measures it. */
export type StripBox = Pick<HTMLElement, "scrollLeft" | "scrollWidth" | "clientWidth">;

/** The left and right edges of a box on screen. */
export type Edges = Pick<DOMRect, "left" | "right">;

/** A wheel turn over the strip. */
export type WheelTurn = Pick<WheelEvent, "deltaX" | "deltaY" | "deltaMode" | "ctrlKey"> & {
	/** Milliseconds since a wheel turn last scrolled something other than a strip. */
	readonly sincePage: number;
};

/** How wide a fade grows, in pixels. */
export const FADE_PX = 14;

// How long a wheel that's scrolling the list stays with it.
const LATCH_MS = 400;

// A wheel turn counted in lines or pages.
const DELTA_LINE = 1;
const DELTA_PAGE = 2;
const LINE_PX = 16;

/** How wide the fade at each end is: as wide as the chips hidden past it, up to `FADE_PX`. */
export const stripFades = ({ scrollLeft, scrollWidth, clientWidth }: StripBox) => {
	const fade = (hidden: number) => Math.min(FADE_PX, Math.max(0, Math.round(hidden)));
	return { start: fade(scrollLeft), end: fade(scrollWidth - clientWidth - scrollLeft) };
};

/** How far a wheel turn scrolls the strip sideways, or null to leave it to the list. */
export const wheelShift = (box: StripBox, turn: WheelTurn): number | null => {
	// Sideways swipes scroll the strip on their own, and Control is a pinch to zoom.
	if (turn.ctrlKey || Math.abs(turn.deltaX) >= Math.abs(turn.deltaY)) return null;
	if (turn.sincePage < LATCH_MS) return null;
	const unit =
		turn.deltaMode === DELTA_LINE
			? LINE_PX
			: turn.deltaMode === DELTA_PAGE
				? box.clientWidth
				: 1;
	const shift = turn.deltaY * unit;
	const room = shift > 0 ? box.scrollWidth - box.clientWidth - box.scrollLeft : box.scrollLeft;
	if (room < 1) return null;
	return Math.sign(shift) * Math.min(Math.abs(shift), room);
};

/** How far to scroll the strip so all of `chip` shows, clear of the fades, or 0 when it does. */
export const revealShift = (strip: Edges, chip: Edges) => {
	if (chip.left < strip.left + FADE_PX) return chip.left - strip.left - FADE_PX;
	if (chip.right > strip.right - FADE_PX) return chip.right - strip.right + FADE_PX;
	return 0;
};

// When a wheel turn last scrolled something other than a strip. A strip
// that takes a turn stops it, so the window hears only the others.
let pageWheelAt = Number.NEGATIVE_INFINITY;
let hearing = false;
const hearPageWheel = (e: WheelEvent) => {
	if (!e.defaultPrevented) pageWheelAt = e.timeStamp;
};

// Wires a strip up when it mounts, and returns what unwires it.
const attach = (strip: HTMLElement | null) => {
	if (!strip) return;
	if (!hearing) {
		window.addEventListener("wheel", hearPageWheel, { passive: true });
		hearing = true;
	}
	let shown = { start: 0, end: 0 };
	const fade = () => {
		const next = stripFades(strip);
		if (next.start !== shown.start) strip.style.setProperty("--fade-start", `${next.start}px`);
		if (next.end !== shown.end) strip.style.setProperty("--fade-end", `${next.end}px`);
		shown = next;
	};
	const wheel = (e: WheelEvent) => {
		const shift = wheelShift(strip, {
			deltaX: e.deltaX,
			deltaY: e.deltaY,
			deltaMode: e.deltaMode,
			ctrlKey: e.ctrlKey,
			sincePage: e.timeStamp - pageWheelAt,
		});
		if (shift === null) return;
		e.preventDefault();
		strip.scrollLeft += shift;
	};
	let pressedAt: number | null = null;
	const press = () => {
		pressedAt = strip.scrollLeft;
	};
	const click = (e: MouseEvent) => {
		const scrolled = pressedAt !== null && Math.abs(strip.scrollLeft - pressedAt) >= 1;
		pressedAt = null;
		if (!scrolled) return;
		e.preventDefault();
		e.stopPropagation();
	};
	// Only focus that shows a ring, so a click never scrolls the strip.
	const reveal = (e: FocusEvent) => {
		const chip = e.target as Element;
		if (!chip.matches(":focus-visible")) return;
		const shift = revealShift(strip.getBoundingClientRect(), chip.getBoundingClientRect());
		if (shift !== 0) strip.scrollLeft += shift;
	};
	// The strip changes size with its row, and the chips with what's linked.
	const resize = new ResizeObserver(fade);
	resize.observe(strip);
	if (strip.firstElementChild) resize.observe(strip.firstElementChild);
	strip.addEventListener("scroll", fade, { passive: true });
	strip.addEventListener("wheel", wheel, { passive: false });
	strip.addEventListener("pointerdown", press);
	strip.addEventListener("click", click, { capture: true });
	strip.addEventListener("focusin", reveal);
	return () => {
		resize.disconnect();
		strip.removeEventListener("scroll", fade);
		strip.removeEventListener("wheel", wheel);
		strip.removeEventListener("pointerdown", press);
		strip.removeEventListener("click", click, { capture: true });
		strip.removeEventListener("focusin", reveal);
	};
};

// The fades are a mask, so they work on any background. The strip is
// relative so the chips' screen reader text, placed absolutely, stays
// inside it, where it would otherwise widen the list.
const STRIP =
	"relative flex min-w-0 overflow-x-auto overflow-y-hidden overscroll-x-contain [scrollbar-width:none] [&::-webkit-scrollbar]:hidden mask-l-from-[calc(100%-var(--fade-start,0px))] mask-r-from-[calc(100%-var(--fade-end,0px))]";

/** A strip of chips, one line, that scrolls sideways when they don't fit. */
export const ChipStrip = ({
	className,
	rings = false,
	children,
}: {
	className?: string;
	/**
	 * Leaves room inside the strip's edges for the chips' focus rings, which
	 * reach 4px past them, and takes it back outside so the chips sit where
	 * they would without it.
	 */
	rings?: boolean;
	children: ReactNode;
}) => (
	<span ref={attach} data-chip-strip="" className={cn(STRIP, rings && "-m-1", className)}>
		<span className={cn("flex shrink-0 items-center gap-1", rings && "p-1")}>{children}</span>
	</span>
);
