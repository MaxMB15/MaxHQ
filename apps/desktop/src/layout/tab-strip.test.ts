// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import { revealTab } from "./tab-strip";

// A strip `width` wide scrolled to `scroll`, holding one tab at `left`,
// `size` wide, with a title inside. jsdom lays nothing out, so the sizes
// are set by hand.
const strip = ({
	width,
	scroll,
	left,
	size,
	kind = "dv-tabs-container",
}: {
	width: number;
	scroll: number;
	left: number;
	size: number;
	kind?: string;
}) => {
	const list = document.createElement("div");
	list.className = kind;
	const tab = document.createElement("div");
	tab.className = "dv-tab";
	const title = document.createElement("span");
	tab.append(title);
	list.append(tab);
	Object.defineProperty(list, "clientWidth", { value: width });
	Object.defineProperty(list, "scrollLeft", { value: scroll, writable: true });
	Object.defineProperty(tab, "offsetLeft", { value: left });
	Object.defineProperty(tab, "offsetWidth", { value: size });
	const scrolled = vi.fn();
	list.addEventListener("scroll", scrolled);
	return { list, title, scrolled };
};

describe("revealTab", () => {
	it("scrolls a tab past the strip's end in until its end shows", () => {
		const { list, title, scrolled } = strip({ width: 300, scroll: 0, left: 280, size: 120 });
		revealTab(title);
		expect(list.scrollLeft).toBe(100);
		expect(scrolled).toHaveBeenCalledTimes(1);
	});

	it("scrolls back to a tab before the strip's start", () => {
		const { list, title } = strip({ width: 300, scroll: 200, left: 50, size: 120 });
		revealTab(title);
		expect(list.scrollLeft).toBe(50);
	});

	it("shows a tab wider than the strip from its start", () => {
		const { list, title } = strip({ width: 100, scroll: 0, left: 280, size: 150 });
		revealTab(title);
		expect(list.scrollLeft).toBe(280);
	});

	it("leaves a tab that shows, or an element outside a tab, alone", () => {
		const { list, title, scrolled } = strip({ width: 300, scroll: 40, left: 60, size: 120 });
		revealTab(title);
		expect(list.scrollLeft).toBe(40);
		revealTab(document.createElement("span"));
		expect(scrolled).not.toHaveBeenCalled();
	});

	it("leaves a tab in the overflow list alone", () => {
		const { list, title, scrolled } = strip({
			width: 300,
			scroll: 0,
			left: 280,
			size: 120,
			kind: "dv-tabs-overflow-dropdown",
		});
		revealTab(title);
		expect(list.scrollLeft).toBe(0);
		expect(scrolled).not.toHaveBeenCalled();
	});
});
