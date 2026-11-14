// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FrameReport } from "@maxhq/shared/visuals/frame";
import { visualKey } from "@maxhq/shared/visuals/visual";

import { VisualFrame } from "./visual-frame";

const HTML = "<p>Hello</p>";

const frameOf = (): HTMLIFrameElement => {
	const frame = document.querySelector("iframe");
	if (!frame) throw new Error("No frame");
	return frame;
};

/** Says something from the frame, as its page would. */
const say = (report: FrameReport, frame = frameOf()) =>
	act(() => {
		window.dispatchEvent(
			new MessageEvent("message", { data: report, source: frame.contentWindow })
		);
	});

afterEach(() => {
	localStorage.clear();
	vi.restoreAllMocks();
});

describe("a visual's frame", () => {
	it("runs its page in a sandbox with scripts only", () => {
		render(<VisualFrame html={HTML} title="Hello" />);
		const frame = frameOf();
		expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
		expect(frame.getAttribute("src")).toMatch(/visual\.html\?theme=(dark|light)$/);
	});

	it("sends the page once the frame is ready, and only once", () => {
		render(<VisualFrame html={HTML} title="Hello" />);
		const post = vi.spyOn(frameOf().contentWindow as Window, "postMessage");
		say({ type: "maxhq-visual:ready" });
		say({ type: "maxhq-visual:ready" });
		expect(post).toHaveBeenCalledTimes(1);
		expect(post.mock.calls[0]?.[0]).toMatchObject({ type: "maxhq-visual:show", html: HTML });
	});

	it("ignores a message from anywhere but its frame", () => {
		render(<VisualFrame html={HTML} title="Hello" />);
		act(() => {
			window.dispatchEvent(
				new MessageEvent("message", {
					data: { type: "maxhq-visual:error", message: "Nope", line: null },
					source: window,
				})
			);
		});
		expect(screen.queryByRole("alert")).toBeNull();
	});

	it("grows with its page, up to the limit", () => {
		render(<VisualFrame html={HTML} title="Hello" />);
		say({ type: "maxhq-visual:height", height: 320 });
		expect(frameOf().style.height).toBe("320px");
		say({ type: "maxhq-visual:height", height: 90_000 });
		expect(frameOf().style.height).toBe("20000px");
	});

	it("shows an error, counts the rest, and tells the agent", () => {
		const onTell = vi.fn();
		render(<VisualFrame html={HTML} title="Hello" onTell={onTell} />);
		say({ type: "maxhq-visual:error", message: "x is not defined", line: 4 });
		say({ type: "maxhq-visual:error", message: "y is not defined", line: null });
		say({ type: "maxhq-visual:error", message: "x is not defined", line: 4 });
		expect(screen.getByRole("alert")).toHaveTextContent(
			"x is not defined (line 4), and 2 more errors"
		);
		fireEvent.click(screen.getByRole("button", { name: "Tell the agent" }));
		expect(onTell).toHaveBeenCalledWith(
			[
				{ message: "x is not defined", line: 4 },
				{ message: "y is not defined", line: null },
			],
			3
		);
	});

	it("has no Tell the agent without an agent to tell", () => {
		render(<VisualFrame html={HTML} title="Hello" />);
		say({ type: "maxhq-visual:error", message: "Oops", line: null });
		expect(screen.queryByRole("button", { name: "Tell the agent" })).toBeNull();
	});

	it("names a missing file, with nothing to press", () => {
		render(<VisualFrame html={HTML} title="Hello" />);
		say({ type: "maxhq-visual:missing", path: "data.json" });
		expect(screen.getByText(/Not found: data\.json/)).toBeInTheDocument();
		expect(screen.queryByRole("button")).toBeNull();
	});

	it("lets a blocked host load this time, in a new frame", () => {
		render(<VisualFrame html={HTML} title="Hello" />);
		say({
			type: "maxhq-visual:blocked",
			url: "https://cdn.example.com/lib.js",
			what: "script-src-elem",
		});
		expect(screen.getByText(/Blocked loading from cdn\.example\.com/)).toBeInTheDocument();
		expect(screen.getByText(/tells them your address/)).toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Load this time" }));
		expect(frameOf().getAttribute("src")).toMatch(/&hosts=cdn\.example\.com$/);
		// The new frame starts with nothing to report.
		expect(screen.queryByText(/Blocked loading/)).toBeNull();
	});

	it("opens a link only while its frame has the focus", () => {
		const open = vi.spyOn(window, "open").mockReturnValue(null);
		render(<VisualFrame html={HTML} title="Hello" />);
		say({ type: "maxhq-visual:link", url: "https://example.com/" });
		expect(open).not.toHaveBeenCalled();
		frameOf().focus();
		say({ type: "maxhq-visual:link", url: "https://example.com/" });
		expect(open).toHaveBeenCalledWith("https://example.com/", "_blank", "noreferrer");
	});

	it("marks its page running until it settles", () => {
		render(<VisualFrame html={HTML} title="Hello" />);
		say({ type: "maxhq-visual:ready" });
		expect(localStorage.getItem("maxhq.visual.drawing")).toBe(visualKey(HTML));
		say({ type: "maxhq-visual:settled" });
		expect(localStorage.getItem("maxhq.visual.drawing")).toBeNull();
	});
});

describe("a visual that was running when the window went away", () => {
	it("waits for Run anyway, then runs", async () => {
		localStorage.setItem("maxhq.visual.drawing", visualKey(HTML));
		vi.resetModules();
		const { VisualFrame: Fresh } = await import("./visual-frame");
		render(<Fresh html={HTML} title="Hello" />);
		expect(document.querySelector("iframe")).toBeNull();
		expect(screen.getByText(/stopped responding/)).toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "View source" }));
		expect(screen.getByText("<p>Hello</p>")).toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Run anyway" }));
		expect(document.querySelector("iframe")).not.toBeNull();
		expect(localStorage.getItem("maxhq.visual.heldBack") ?? "").not.toContain(visualKey(HTML));
	});

	it("leaves other visuals alone", async () => {
		localStorage.setItem("maxhq.visual.drawing", visualKey("<p>Other</p>"));
		vi.resetModules();
		const { VisualFrame: Fresh } = await import("./visual-frame");
		render(<Fresh html={HTML} title="Hello" />);
		expect(document.querySelector("iframe")).not.toBeNull();
	});
});
