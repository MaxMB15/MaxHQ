// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Account } from "../bindings";
import { account, device, entities } from "../test/build";
import { applySnapshot } from "../view/replica";

import { effortDefaultRows, effortDefaultTitle } from "./effort-defaults";

const view = (accounts: Account[]) =>
	applySnapshot({
		seq: 1,
		entities: entities({ devices: [device({ id: "mac", os: "macos" })], accounts }),
	});

const CLAUDE = account({
	id: "claude",
	device: "mac",
	models: [
		{ id: "opus", label: "Opus", default: true, efforts: ["low", "medium", "high", "max"] },
	],
});

const CODEX = account({
	id: "codex",
	device: "mac",
	vendor: "codex",
	models: [
		{
			id: "gpt",
			label: "GPT",
			default: true,
			efforts: ["low", "medium", "high"],
			default_effort: "medium",
		},
	],
});

const NONE = { claude_effort: null, codex_effort: null };

describe("effortDefaultRows", () => {
	it("gives Claude Code then Codex, each at the level new agents start at", () => {
		const rows = effortDefaultRows(view([CODEX, CLAUDE]), "mac", NONE);
		expect(rows.map((r) => [r.vendor, r.field, r.title, r.level])).toEqual([
			["claude", "claude_effort", "Claude Code effort", "high"],
			["codex", "codex_effort", "Codex effort", "medium"],
		]);
		expect(rows[1]?.sub).toBe("The same for new Codex agents.");
		expect(rows[1]?.choices.find((c) => c.isDefault)?.value).toBe("medium");
	});

	it("shows the device's own level, and keeps one its logins no longer list", () => {
		const [claude] = effortDefaultRows(view([CLAUDE]), "mac", {
			...NONE,
			claude_effort: "ultra",
		});
		expect(claude?.level).toBe("ultra");
		expect(claude?.choices.map((c) => c.value)).toContain("ultra");
	});

	it("leaves out a vendor no login there lists levels for", () => {
		expect(effortDefaultRows(view([CLAUDE]), "mac", NONE).map((r) => r.vendor)).toEqual([
			"claude",
		]);
		expect(effortDefaultRows(view([]), "mac", NONE)).toEqual([]);
	});

	it("names each vendor's row", () => {
		expect(effortDefaultTitle("claude")).toBe("Claude Code effort");
		expect(effortDefaultTitle("codex")).toBe("Codex effort");
	});
});
