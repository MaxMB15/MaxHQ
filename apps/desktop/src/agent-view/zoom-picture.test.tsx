// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ZoomPicture } from "./zoom-picture";

// A frame 800 by 600 and a picture 3200 by 1200, which fits at 25%.
beforeEach(() => {
	vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(800);
	vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(600);
	vi.spyOn(HTMLImageElement.prototype, "naturalWidth", "get").mockReturnValue(3200);
	vi.spyOn(HTMLImageElement.prototype, "naturalHeight", "get").mockReturnValue(1200);
});

afterEach(() => vi.restoreAllMocks());

const show = (props: Partial<Parameters<typeof ZoomPicture>[0]> = {}) => {
	const shown = render(
		<ZoomPicture src="data:image/png;x" alt="shot.png" keys="window" {...props} />
	);
	const image = screen.getByRole("img", { name: "shot.png" });
	fireEvent.load(image);
	return { ...shown, image, frame: image.parentElement! };
};

const level = () => screen.getByRole("button", { name: "Actual size" }).textContent;

describe("a picture that zooms", () => {
	it("opens fitted and centred", () => {
		const { image } = show();
		expect(level()).toBe("25%");
		expect(image.style.transform).toBe("translate3d(0px, 150px, 0) scale(0.25)");
	});

	it("zooms with the keys and the control, and fits again", () => {
		const { frame } = show();
		fireEvent.keyDown(document.body, { key: "1" });
		expect(level()).toBe("100%");
		expect(frame.className).toContain("cursor-grab");
		fireEvent.keyDown(document.body, { key: "+" });
		expect(level()).toBe("125%");
		fireEvent.click(screen.getByRole("button", { name: "Zoom out" }));
		expect(level()).toBe("100%");
		fireEvent.keyDown(document.body, { key: "0" });
		expect(level()).toBe("25%");
		expect(frame.className).not.toContain("cursor-grab");
	});

	it("goes between fitted and actual size on a double click", () => {
		const { image } = show();
		fireEvent.doubleClick(image, { clientX: 400, clientY: 300 });
		expect(level()).toBe("100%");
		fireEvent.doubleClick(image, { clientX: 400, clientY: 300 });
		expect(level()).toBe("25%");
	});

	it("zooms on a pinch around the pointer, and pans on a plain scroll once bigger", () => {
		const { frame, image } = show();
		fireEvent.wheel(frame, { deltaY: -50, ctrlKey: true, clientX: 0, clientY: 0 });
		expect(level()).toBe("41%");
		fireEvent.keyDown(document.body, { key: "1" });
		const before = image.style.transform;
		fireEvent.wheel(frame, { deltaX: 40, deltaY: 0 });
		expect(image.style.transform).not.toBe(before);
	});

	it("pans on a drag once bigger than the frame, and doesn't count it as a click", () => {
		const onBackground = vi.fn();
		const { frame, image } = show({ onBackground });
		fireEvent.keyDown(document.body, { key: "1" });
		const before = image.style.transform;
		frame.setPointerCapture = vi.fn();
		fireEvent.pointerDown(frame, { button: 0, clientX: 100, clientY: 100, pointerId: 1 });
		expect(frame.className).toContain("cursor-grabbing");
		fireEvent.pointerMove(frame, { clientX: 60, clientY: 100, pointerId: 1 });
		fireEvent.pointerUp(frame, { pointerId: 1 });
		fireEvent.click(frame);
		expect(image.style.transform).not.toBe(before);
		expect(onBackground).not.toHaveBeenCalled();
	});

	it("says a click around the picture, so a viewer can close", () => {
		const onBackground = vi.fn();
		const { frame, image } = show({ onBackground });
		fireEvent.click(image);
		expect(onBackground).not.toHaveBeenCalled();
		fireEvent.click(frame);
		expect(onBackground).toHaveBeenCalledOnce();
	});

	it("takes keys only while focused when asked to", () => {
		const { frame } = show({ keys: "focus" });
		fireEvent.keyDown(document.body, { key: "1" });
		expect(level()).toBe("25%");
		fireEvent.keyDown(frame, { key: "1" });
		expect(level()).toBe("100%");
	});
});
