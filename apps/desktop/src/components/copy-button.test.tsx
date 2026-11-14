// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CopyButton } from "./copy-button";

const clipboard = (writeText: (text: string) => Promise<void>) =>
	Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });

// Lets the clipboard's promise settle while the clock stands still.
const settle = () => act(async () => {});

beforeEach(() => {
	vi.useFakeTimers();
	vi.spyOn(console, "warn").mockImplementation(() => undefined);
});

afterEach(() => {
	vi.useRealTimers();
	vi.restoreAllMocks();
	Reflect.deleteProperty(navigator, "clipboard");
});

describe("CopyButton", () => {
	it("says Copied for a moment, then goes back", async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		clipboard(writeText);
		render(<CopyButton text="claude setup-token" label="Copy command" />);

		fireEvent.click(screen.getByRole("button", { name: "Copy command" }));
		expect(writeText).toHaveBeenCalledWith("claude setup-token");
		await settle();
		expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
		expect(screen.getByRole("status")).toHaveTextContent("Copied");

		act(() => vi.advanceTimersByTime(1400));
		expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
		act(() => vi.advanceTimersByTime(100));
		expect(screen.getByRole("button", { name: "Copy command" })).toBeInTheDocument();
		expect(screen.getByRole("status")).toHaveTextContent("");
	});

	it("starts the wait again on a second click", async () => {
		clipboard(vi.fn().mockResolvedValue(undefined));
		render(<CopyButton text="ssh devbox" />);
		const button = screen.getByRole("button", { name: "Copy" });

		fireEvent.click(button);
		await settle();
		act(() => vi.advanceTimersByTime(1000));
		fireEvent.click(button);
		await settle();
		act(() => vi.advanceTimersByTime(1000));
		expect(button).toHaveTextContent(/^Copied$/);
		act(() => vi.advanceTimersByTime(500));
		expect(button).toHaveTextContent(/^Copy$/);
	});

	it("says it couldn't copy when the clipboard refuses", async () => {
		clipboard(vi.fn().mockRejectedValue(new Error("Not allowed")));
		render(<CopyButton text="ssh devbox" />);

		fireEvent.click(screen.getByRole("button", { name: "Copy" }));
		await settle();
		expect(screen.getByRole("button", { name: "Couldn't copy" })).toBeInTheDocument();
		expect(screen.getByRole("status")).toHaveTextContent("Couldn't copy");
		expect(screen.queryByText("Copied")).toBeNull();

		act(() => vi.advanceTimersByTime(3000));
		expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
	});

	it("says it couldn't copy when there's no clipboard", async () => {
		Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
		render(<CopyButton text="ssh devbox" />);

		fireEvent.click(screen.getByRole("button", { name: "Copy" }));
		await settle();
		expect(screen.getByRole("button", { name: "Couldn't copy" })).toBeInTheDocument();
	});
});
