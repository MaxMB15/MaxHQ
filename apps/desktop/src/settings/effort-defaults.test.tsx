// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { EFFORT_SINCE } from "@maxhq/shared/bindings";
import { account, device, entities, link } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import { EffortDefaultRows } from "./effort-defaults";

const view = (protocol: number) =>
	applySnapshot({
		seq: 1,
		entities: entities({
			devices: [device({ id: "mac", name: "Mac" })],
			links: [link({ id: "local", device: "mac", protocol })],
			accounts: [
				account({
					id: "claude",
					device: "mac",
					models: [
						{
							id: "opus",
							label: "Opus",
							default: true,
							efforts: ["low", "medium", "max"],
							default_effort: "medium",
						},
					],
				}),
			],
		}),
	});

const rows = (protocol: number, onSave = vi.fn(), claude: string | null = "max") =>
	render(
		<EffortDefaultRows
			view={view(protocol)}
			device="mac"
			name="Mac"
			live
			shown={{ claude_effort: claude, codex_effort: null }}
			note={null}
			onSave={onSave}
		/>
	);

describe("effort defaults", () => {
	it("shows a vendor with levels on a slider and saves the level picked", () => {
		const onSave = vi.fn();
		rows(EFFORT_SINCE, onSave);
		expect(screen.getByText("Claude Code effort")).toBeInTheDocument();
		// No Codex login here lists levels.
		expect(screen.queryByText("Codex effort")).not.toBeInTheDocument();
		const slider = screen.getByRole("slider", { name: "Claude Code effort" });
		expect(slider).toHaveAttribute("aria-valuetext", "Max");

		fireEvent.keyDown(slider, { key: "Home" });
		expect(onSave).toHaveBeenCalledWith({ claude_effort: "low" });
	});

	it("shows the level agents start at before one is picked", () => {
		rows(EFFORT_SINCE, vi.fn(), null);
		const slider = screen.getByRole("slider", { name: "Claude Code effort" });
		expect(slider).toHaveAttribute("aria-valuetext", "Medium (default)");
		expect(screen.queryByText(/Vendor default/)).not.toBeInTheDocument();
	});

	it("can't change on a maxd from before effort levels", () => {
		rows(EFFORT_SINCE - 1);
		expect(
			screen.getByText("Mac's maxd is too old for this. Update it first.")
		).toBeInTheDocument();
		expect(screen.getByRole("slider")).toHaveAttribute("aria-disabled", "true");
	});
});
