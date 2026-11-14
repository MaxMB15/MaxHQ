// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { spanKind, StepText } from "./login-steps";

describe("a step's code spans", () => {
	it("tells links, things to type and plain code apart", () => {
		expect(spanKind("https://cursor.com/dashboard")).toBe("link");
		expect(spanKind("https://github.com/<your GitHub login>")).toBe("copy");
		expect(spanKind("claude setup-token")).toBe("copy");
		expect(spanKind("CLAUDE_CONFIG_DIR=<dir> claude setup-token")).toBe("copy");
		expect(spanKind("MaxHQ pushes")).toBe("copy");
		expect(spanKind("secrets/cursor.json")).toBe("code");
	});

	it("draws a web address as a link, and gives it and a command a copy icon", async () => {
		const writeText = vi.fn(async () => {});
		Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
		render(
			<p>
				<StepText text="Open `https://example.com/keys`, then run `claude setup-token` in `~/x`." />
			</p>
		);
		const link = screen.getByRole("link", { name: "https://example.com/keys" });
		expect(link).toHaveAttribute("href", "https://example.com/keys");
		expect(link).toHaveAttribute("target", "_blank");
		expect(screen.getAllByRole("button")).toHaveLength(2);
		await act(async () => {
			fireEvent.click(screen.getByRole("button", { name: "Copy URL" }));
		});
		expect(writeText).toHaveBeenCalledWith("https://example.com/keys");
		await act(async () => {
			fireEvent.click(screen.getByRole("button", { name: "Copy" }));
		});
		expect(writeText).toHaveBeenCalledWith("claude setup-token");
		Reflect.deleteProperty(navigator, "clipboard");
	});
});
