// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { watchWindowScroll } from "./window-scroll";

// The test window has no layout, so its scroll offsets are set by hand.
const scrolledTo = (x: number, y: number) => {
	Object.defineProperty(window, "scrollX", { value: x, configurable: true });
	Object.defineProperty(window, "scrollY", { value: y, configurable: true });
};

afterEach(() => {
	scrolledTo(0, 0);
	vi.restoreAllMocks();
});

describe("the window's scroll", () => {
	it("goes back to the corner when something scrolls the window, until stopped", () => {
		const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
		const stop = watchWindowScroll();

		// A focus near the bottom slid the app up.
		scrolledTo(0, 20);
		window.dispatchEvent(new Event("scroll"));
		expect(scrollTo).toHaveBeenCalledExactlyOnceWith(0, 0);

		// Back in the corner, the scroll that put it there changes nothing.
		scrolledTo(0, 0);
		window.dispatchEvent(new Event("scroll"));
		expect(scrollTo).toHaveBeenCalledTimes(1);

		stop();
		scrolledTo(12, 0);
		window.dispatchEvent(new Event("scroll"));
		expect(scrollTo).toHaveBeenCalledTimes(1);
	});
});
