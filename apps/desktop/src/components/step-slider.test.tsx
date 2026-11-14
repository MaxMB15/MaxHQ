// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { StepSlider, type Stop } from "./step-slider";

const STOPS: Stop[] = [
	{ value: "low", label: "Low" },
	{ value: "medium", label: "Medium", isDefault: true },
	{ value: "high", label: "High" },
	{ value: "max", label: "Max" },
];

const slider = (value = "medium", onPick = vi.fn(), onDone?: () => void) => {
	const view = render(
		<StepSlider label="Effort" stops={STOPS} value={value} onPick={onPick} onDone={onDone} />
	);
	return { ...view, onPick, thumb: screen.getByRole("slider", { name: "Effort" }) };
};

/** Lays the slider out 400 pixels wide from x 100, so each stop is 100 wide. */
const layOut = (container: HTMLElement) => {
	const root = container.firstElementChild as HTMLElement;
	root.getBoundingClientRect = () =>
		({ left: 100, width: 400, top: 0, height: 40, right: 500, bottom: 40 }) as DOMRect;
	return root;
};

describe("StepSlider", () => {
	it("names the level and marks the default", () => {
		const { thumb } = slider("high");
		expect(thumb).toHaveAttribute("aria-valuetext", "High");
		expect(thumb).toHaveAttribute("aria-valuenow", "2");
		expect(thumb).toHaveAttribute("aria-valuemax", "3");
		expect(screen.getByText("Default")).toBeInTheDocument();
	});

	it("says when the level is the default", () => {
		const { thumb } = slider("medium");
		expect(thumb).toHaveAttribute("aria-valuetext", "Medium (default)");
	});

	it("moves with the arrow keys, Home and End, picking each level", () => {
		const { thumb, onPick } = slider("medium");
		fireEvent.keyDown(thumb, { key: "ArrowRight" });
		expect(onPick).toHaveBeenLastCalledWith("high");
		// The thumb stays on the pick while it's on its way.
		expect(thumb).toHaveAttribute("aria-valuetext", "High");
		fireEvent.keyDown(thumb, { key: "ArrowUp" });
		expect(onPick).toHaveBeenLastCalledWith("max");
		fireEvent.keyDown(thumb, { key: "Home" });
		expect(onPick).toHaveBeenLastCalledWith("low");
		fireEvent.keyDown(thumb, { key: "ArrowLeft" });
		fireEvent.keyDown(thumb, { key: "ArrowDown" });
		// Already at the first level: nothing more to send.
		expect(onPick).toHaveBeenCalledTimes(3);
		fireEvent.keyDown(thumb, { key: "End" });
		expect(onPick).toHaveBeenLastCalledWith("max");
	});

	it("closes on Enter or Escape", () => {
		const onDone = vi.fn();
		const { thumb } = slider("medium", vi.fn(), onDone);
		fireEvent.keyDown(thumb, { key: "Enter" });
		fireEvent.keyDown(thumb, { key: "Escape" });
		expect(onDone).toHaveBeenCalledTimes(2);
	});

	it("picks the stop clicked", () => {
		const { container, onPick, thumb } = slider("medium");
		const root = layOut(container);
		fireEvent.pointerDown(root, { button: 0, pointerId: 1, clientX: 470 });
		fireEvent.pointerUp(root, { pointerId: 1, clientX: 470 });
		expect(onPick).toHaveBeenCalledExactlyOnceWith("max");
		expect(thumb).toHaveFocus();
	});

	it("follows a drag and picks where it lets go", () => {
		const { container, onPick, thumb } = slider("medium");
		const root = layOut(container);
		fireEvent.pointerDown(root, { button: 0, pointerId: 1, clientX: 250 });
		fireEvent.pointerMove(root, { pointerId: 1, clientX: 120 });
		expect(thumb).toHaveAttribute("aria-valuetext", "Low");
		// Past the end it stays on the last stop.
		fireEvent.pointerMove(root, { pointerId: 1, clientX: 900 });
		expect(thumb).toHaveAttribute("aria-valuetext", "Max");
		expect(onPick).not.toHaveBeenCalled();
		fireEvent.pointerUp(root, { pointerId: 1, clientX: 330 });
		expect(onPick).toHaveBeenCalledExactlyOnceWith("high");
	});

	it("sends nothing for the level it has", () => {
		const { container, onPick } = slider("medium");
		const root = layOut(container);
		fireEvent.pointerDown(root, { button: 0, pointerId: 1, clientX: 250 });
		fireEvent.pointerUp(root, { pointerId: 1, clientX: 250 });
		expect(onPick).not.toHaveBeenCalled();
	});

	it("follows a new value from outside", () => {
		const onPick = vi.fn();
		const { rerender, thumb } = slider("medium", onPick);
		fireEvent.keyDown(thumb, { key: "ArrowRight" });
		rerender(<StepSlider label="Effort" stops={STOPS} value="low" onPick={onPick} />);
		expect(thumb).toHaveAttribute("aria-valuetext", "Low");
	});

	it("can't move while disabled", () => {
		const onPick = vi.fn();
		render(<StepSlider label="Effort" stops={STOPS} value="low" onPick={onPick} disabled />);
		const thumb = screen.getByRole("slider");
		expect(thumb).toHaveAttribute("aria-disabled", "true");
		expect(thumb).toHaveAttribute("tabindex", "-1");
		fireEvent.keyDown(thumb, { key: "End" });
		expect(onPick).not.toHaveBeenCalled();
	});
});
