// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ErrorBoundary } from "./error-boundary";

// Throws while `broken` says so, and counts how often it mounts.
let broken = true;
let mounts = 0;
const Part = () => {
	useState(() => (mounts += 1));
	if (broken) throw new Error("the part broke");
	return <p>The part works</p>;
};

let logged: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
	broken = true;
	mounts = 0;
	logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => vi.restoreAllMocks());

describe("ErrorBoundary", () => {
	it("shows its children while they work", () => {
		broken = false;
		render(
			<ErrorBoundary part="composer">
				<Part />
			</ErrorBoundary>
		);
		expect(screen.getByText("The part works")).toBeInTheDocument();
		expect(screen.queryByRole("alert")).toBeNull();
	});

	it("says the part hit an error, in its spot, and logs it", () => {
		render(
			<>
				<p>The rest of the window</p>
				<ErrorBoundary part="composer">
					<Part />
				</ErrorBoundary>
			</>
		);
		expect(screen.getByRole("alert")).toHaveTextContent("This part of MaxHQ hit an error.");
		expect(screen.getByText("The rest of the window")).toBeInTheDocument();
		expect(logged).toHaveBeenCalledWith(
			"The composer hit an error",
			expect.objectContaining({ message: "the part broke" }),
			expect.any(String)
		);
	});

	it("mounts the part again when you try again", () => {
		render(
			<ErrorBoundary part="timeline" look="strip">
				<Part />
			</ErrorBoundary>
		);
		const before = mounts;
		broken = false;
		fireEvent.click(screen.getByRole("button", { name: "Try again" }));
		expect(screen.getByText("The part works")).toBeInTheDocument();
		expect(screen.queryByRole("alert")).toBeNull();
		expect(mounts).toBeGreaterThan(before);
	});

	it("starts a part that failed after it mounted from scratch", () => {
		const Counter = () => {
			const [count, setCount] = useState(0);
			if (count === 2) throw new Error("two is too many");
			return (
				<button type="button" onClick={() => setCount(count + 1)}>
					{`Count ${count}`}
				</button>
			);
		};
		render(
			<ErrorBoundary part="agent pane">
				<Counter />
			</ErrorBoundary>
		);
		fireEvent.click(screen.getByRole("button", { name: "Count 0" }));
		fireEvent.click(screen.getByRole("button", { name: "Count 1" }));
		fireEvent.click(screen.getByRole("button", { name: "Try again" }));
		expect(screen.getByRole("button", { name: "Count 0" })).toBeInTheDocument();
	});

	it("shows the line again when the part fails again", () => {
		render(
			<ErrorBoundary part="sidebar">
				<Part />
			</ErrorBoundary>
		);
		fireEvent.click(screen.getByRole("button", { name: "Try again" }));
		expect(screen.getByRole("alert")).toHaveTextContent("This part of MaxHQ hit an error.");
	});
});
