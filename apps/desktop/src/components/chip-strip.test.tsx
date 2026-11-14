// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
	ChipStrip,
	FADE_PX,
	revealShift,
	type StripBox,
	stripFades,
	type WheelTurn,
	wheelShift,
} from "./chip-strip";

// A strip 100 wide holding 300 of chips.
const box = (scrollLeft: number, scrollWidth = 300): StripBox => ({
	scrollLeft,
	scrollWidth,
	clientWidth: 100,
});

const turn = (deltaY: number, more: Partial<WheelTurn> = {}): WheelTurn => ({
	deltaX: 0,
	deltaY,
	deltaMode: 0,
	ctrlKey: false,
	sincePage: Number.POSITIVE_INFINITY,
	...more,
});

describe("stripFades", () => {
	it("shows no fade when every chip fits", () => {
		expect(stripFades(box(0, 100))).toEqual({ start: 0, end: 0 });
	});

	it("fades only the ends with chips hidden past them", () => {
		expect(stripFades(box(0))).toEqual({ start: 0, end: FADE_PX });
		expect(stripFades(box(100))).toEqual({ start: FADE_PX, end: FADE_PX });
		expect(stripFades(box(200))).toEqual({ start: FADE_PX, end: 0 });
	});

	it("grows a fade with the first pixels hidden, in whole pixels", () => {
		expect(stripFades(box(4.4))).toEqual({ start: 4, end: FADE_PX });
		expect(stripFades(box(195))).toEqual({ start: FADE_PX, end: 5 });
	});
});

describe("wheelShift", () => {
	it("turns a vertical wheel into a sideways scroll, either way", () => {
		expect(wheelShift(box(50), turn(40))).toBe(40);
		expect(wheelShift(box(50), turn(-40))).toBe(-40);
	});

	it("scrolls no further than the end", () => {
		expect(wheelShift(box(180), turn(100))).toBe(20);
		expect(wheelShift(box(30), turn(-100))).toBe(-30);
	});

	it("leaves the wheel to the list at the end it turns toward, or when every chip fits", () => {
		expect(wheelShift(box(200), turn(40))).toBeNull();
		expect(wheelShift(box(0), turn(-40))).toBeNull();
		expect(wheelShift(box(0, 100), turn(40))).toBeNull();
	});

	it("leaves sideways swipes and pinches to the browser", () => {
		expect(wheelShift(box(50), turn(10, { deltaX: 30 }))).toBeNull();
		expect(wheelShift(box(50), turn(0, { deltaX: 0 }))).toBeNull();
		expect(wheelShift(box(50), turn(40, { ctrlKey: true }))).toBeNull();
	});

	it("counts a wheel turn in lines and pages", () => {
		expect(wheelShift(box(0), turn(3, { deltaMode: 1 }))).toBe(48);
		expect(wheelShift(box(0), turn(1, { deltaMode: 2 }))).toBe(100);
	});

	it("leaves a wheel that's scrolling the list with it", () => {
		expect(wheelShift(box(50), turn(40, { sincePage: 100 }))).toBeNull();
		expect(wheelShift(box(50), turn(40, { sincePage: 400 }))).toBe(40);
	});
});

describe("revealShift", () => {
	// A strip from 0 to 100 on screen.
	const strip = { left: 0, right: 100 };

	it("leaves a chip that shows clear of the fades", () => {
		expect(revealShift(strip, { left: FADE_PX, right: 100 - FADE_PX })).toBe(0);
	});

	it("scrolls a chip hidden at either end into view, clear of the fade there", () => {
		expect(revealShift(strip, { left: -30, right: 10 })).toBe(-30 - FADE_PX);
		expect(revealShift(strip, { left: 5, right: 40 })).toBe(5 - FADE_PX);
		expect(revealShift(strip, { left: 90, right: 130 })).toBe(30 + FADE_PX);
	});
});

/** Turns the wheel over `target` at `at` milliseconds, and says whether the strip took it. */
const wheelAt = (target: Element, init: WheelEventInit, at: number) => {
	const e = new WheelEvent("wheel", { bubbles: true, cancelable: true, ...init });
	Object.defineProperty(e, "timeStamp", { value: at });
	target.dispatchEvent(e);
	return e.defaultPrevented;
};

/** Gives `strip` a size, since jsdom lays nothing out. */
const sizeStrip = (strip: HTMLElement, scrollWidth: number, clientWidth: number) => {
	Object.defineProperty(strip, "scrollWidth", { configurable: true, value: scrollWidth });
	Object.defineProperty(strip, "clientWidth", { configurable: true, value: clientWidth });
};

describe("ChipStrip", () => {
	const strip = () => {
		const pick = vi.fn();
		render(
			<div onClick={pick}>
				<ChipStrip>
					<span>#141</span>
					<span>#145</span>
				</ChipStrip>
			</div>
		);
		const el = screen.getByText("#141").closest<HTMLElement>("[data-chip-strip]")!;
		sizeStrip(el, 300, 100);
		return { el, pick };
	};

	it("scrolls sideways on a vertical wheel until its end, then lets the list scroll", () => {
		const { el } = strip();
		expect(wheelAt(el, { deltaY: 150 }, 10_000)).toBe(true);
		expect(el.scrollLeft).toBe(150);
		expect(wheelAt(el, { deltaY: 150 }, 11_000)).toBe(true);
		expect(el.scrollLeft).toBe(200);
		expect(wheelAt(el, { deltaY: 150 }, 12_000)).toBe(false);
		expect(el.scrollLeft).toBe(200);
	});

	it("lets a wheel that's scrolling the list pass over it", () => {
		const { el } = strip();
		expect(wheelAt(document.body, { deltaY: 40 }, 20_000)).toBe(false);
		expect(wheelAt(el, { deltaY: 40 }, 20_100)).toBe(false);
		expect(el.scrollLeft).toBe(0);
		expect(wheelAt(el, { deltaY: 40 }, 20_500)).toBe(true);
	});

	it("fades the ends with chips hidden past them as it scrolls", () => {
		const { el } = strip();
		el.scrollLeft = 100;
		fireEvent.scroll(el);
		expect(el.style.getPropertyValue("--fade-start")).toBe(`${FADE_PX}px`);
		expect(el.style.getPropertyValue("--fade-end")).toBe(`${FADE_PX}px`);
		el.scrollLeft = 200;
		fireEvent.scroll(el);
		expect(el.style.getPropertyValue("--fade-end")).toBe("0px");
	});

	it("doesn't count a press that scrolled it as a click", () => {
		const { el, pick } = strip();
		const chip = screen.getByText("#145");
		fireEvent.pointerDown(chip);
		el.scrollLeft = 60;
		fireEvent.click(chip);
		expect(pick).not.toHaveBeenCalled();
		fireEvent.pointerDown(chip);
		fireEvent.click(chip);
		expect(pick).toHaveBeenCalledTimes(1);
	});

	it("scrolls a chip keyboard focus reaches all into view, but not one a press focuses", () => {
		render(
			<ChipStrip>
				<button type="button">#141</button>
				<button type="button">#145</button>
			</ChipStrip>
		);
		const chip = screen.getByRole("button", { name: "#145" });
		const el = chip.closest<HTMLElement>("[data-chip-strip]")!;
		sizeStrip(el, 300, 100);
		// jsdom lays nothing out, so the chip sits half past the strip's end.
		vi.spyOn(el, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 100, 22));
		vi.spyOn(chip, "getBoundingClientRect").mockReturnValue(new DOMRect(80, 0, 60, 22));

		fireEvent.mouseDown(chip);
		chip.focus();
		expect(el.scrollLeft).toBe(0);
		chip.blur();

		fireEvent.keyDown(document.body, { key: "Tab" });
		chip.focus();
		expect(el.scrollLeft).toBe(40 + FADE_PX);
	});

	it("leaves room inside its edges for focus rings only when asked", () => {
		render(
			<>
				<ChipStrip>
					<span>#141</span>
				</ChipStrip>
				<ChipStrip rings>
					<span>#145</span>
				</ChipStrip>
			</>
		);
		const [plain, roomy] = ["#141", "#145"].map((text) =>
			screen.getByText(text).closest<HTMLElement>("[data-chip-strip]")!
		);
		expect(plain).not.toHaveClass("-m-1");
		expect(plain!.firstElementChild).not.toHaveClass("p-1");
		// The rings reach 4px past a chip. The room sits inside the strip,
		// which gives it back outside so the chips don't move.
		expect(roomy).toHaveClass("-m-1");
		expect(roomy!.firstElementChild).toHaveClass("p-1");
	});
});
