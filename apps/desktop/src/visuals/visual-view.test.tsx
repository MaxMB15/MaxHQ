// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { visualActions } from "./actions";
import { clearVisualPages, keepVisualPage } from "./tab-pages";
import { zoomedFrame } from "./visual-frame";
import { VisualPaneFor, VisualView } from "./visual-view";

const VISUAL = {
	html: "<h1>Welcome back</h1>\n<p>Sign in</p>",
	title: "Login flow mockup",
	from: "Usage report",
};

const box = () => document.querySelector<HTMLElement>("[data-visual-box]")!;
const frame = () => document.querySelector("iframe")!;

afterEach(() => {
	clearVisualPages();
	localStorage.clear();
	vi.restoreAllMocks();
});

describe("a visual in a tab", () => {
	it("names the visual and where it came from", () => {
		render(<VisualView visual={VISUAL} />);
		expect(screen.getByRole("heading")).toHaveTextContent("Login flow mockup");
		expect(screen.getByText("from Usage report")).toBeTruthy();
	});

	it("fits the pane at first, and takes a phone's or a tablet's width", () => {
		render(<VisualView visual={VISUAL} />);
		expect(box().style.width).toBe("100%");
		expect(screen.getByRole("button", { name: "Fit" })).toHaveAttribute("aria-pressed", "true");
		fireEvent.click(screen.getByRole("button", { name: "Phone 390" }));
		expect(box().style.width).toBe("390px");
		fireEvent.click(screen.getByRole("button", { name: "Tablet 768" }));
		expect(box().style.width).toBe("768px");
		expect(screen.getByRole("button", { name: "Tablet 768" })).toHaveAttribute(
			"aria-pressed",
			"true"
		);
		fireEvent.click(screen.getByRole("button", { name: "Fit" }));
		expect(box().style.width).toBe("100%");
	});

	it("fills the pane rather than growing with its page", () => {
		render(<VisualView visual={VISUAL} />);
		expect(frame().style.height).toBe("100%");
		expect(frame().style.transform).toBe("");
	});

	it("zooms by laying the page out in less room and scaling it up", () => {
		render(<VisualView visual={VISUAL} />);
		const before = frame();
		fireEvent.change(screen.getByRole("combobox", { name: "Zoom" }), {
			target: { value: "2" },
		});
		expect(frame().style.width).toBe("50%");
		expect(frame().style.height).toBe("50%");
		expect(frame().style.transform).toBe("scale(2)");
		// The page keeps running in the same frame.
		expect(frame()).toBe(before);
	});

	it("lays a page zoomed out in more room", () => {
		expect(zoomedFrame(0.5)).toMatchObject({
			width: "200%",
			height: "200%",
			transform: "scale(0.5)",
		});
	});

	it("shows the page as code for Source, and the page again after", () => {
		render(<VisualView visual={VISUAL} />);
		fireEvent.click(screen.getByRole("button", { name: "</> Source" }));
		expect(document.querySelector("iframe")).toBeNull();
		const code = screen.getByRole("region", { name: /login-flow-mockup\.html/ });
		expect(code).toHaveTextContent("Welcome back");
		expect(screen.getByRole("button", { name: "Phone 390" })).toBeDisabled();
		fireEvent.click(screen.getByRole("button", { name: "</> Source" }));
		expect(document.querySelector("iframe")).not.toBeNull();
	});
});

describe("a visual's pane", () => {
	it("shows the page kept under its key", () => {
		const key = keepVisualPage(VISUAL);
		const close = vi.fn();
		render(<VisualPaneFor pageKey={key} close={close} />);
		expect(screen.getByRole("heading")).toHaveTextContent("Login flow mockup");
		expect(close).not.toHaveBeenCalled();
	});

	it("closes when this window doesn't have its page, as after a restart", () => {
		const close = vi.fn();
		const { container } = render(<VisualPaneFor pageKey="gone" close={close} />);
		expect(close).toHaveBeenCalledTimes(1);
		expect(container).toBeEmptyDOMElement();
	});
});

describe("opening a visual in a tab", () => {
	it("is the first thing a card offers, and only where it can", () => {
		const open = vi.fn();
		const offered = visualActions(VISUAL, { reload: () => {}, open, inApp: false });
		expect(offered.map((a) => a.label)).toEqual(["Open in a tab", "Copy source", "Reload"]);
		offered[0]!.run();
		expect(open).toHaveBeenCalledTimes(1);
		expect(
			visualActions(VISUAL, { reload: () => {}, inApp: false }).map((a) => a.label)
		).not.toContain("Open in a tab");
	});
});
