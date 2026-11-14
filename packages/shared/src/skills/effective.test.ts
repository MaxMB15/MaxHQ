// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { EffectiveSkill } from "../bindings";

import {
	budgetOf,
	cliLabel,
	FALLBACK_LISTING,
	listingLimit,
	MAX_ENTRY_DESCRIPTION,
	markLabel,
	originLabel,
	pushesOut,
	skillsByCli,
} from "./effective";

const skill = (over: Partial<EffectiveSkill> & { name: string }): EffectiveSkill => ({
	description: null,
	path: `/work/repo/.claude/skills/${over.name}/SKILL.md`,
	scope: null,
	enabled: true,
	display_name: null,
	short_description: null,
	user_invocation_only: null,
	user_invocable: null,
	place: "project",
	cli: "claude",
	mark: null,
	...over,
});

describe("effective skill rows", () => {
	it("groups by CLI, then by place and name, leaving out CLIs that see nothing", () => {
		const groups = skillsByCli([
			skill({ name: "zeta", cli: "codex", place: "system" }),
			skill({ name: "beta", place: "global" }),
			skill({ name: "alpha", place: "global" }),
			skill({ name: "omega", place: "project" }),
			skill({ name: "unslop", cli: "codex", place: "always" }),
		]);
		expect(groups.map((g) => [cliLabel(g.cli), g.skills.map((s) => s.name)])).toEqual([
			["Claude", ["omega", "alpha", "beta"]],
			["Codex", ["unslop", "zeta"]],
		]);
	});

	it("says where a skill comes from and what its mark means", () => {
		expect(originLabel(skill({ name: "a", mark: "vendored" }))).toBe("Vendored");
		expect(originLabel(skill({ name: "a", place: "global", scope: "synced" }))).toBe("Synced");
		expect(originLabel(skill({ name: "a", place: "always" }))).toBe("Every agent");
		expect(originLabel(skill({ name: "a", place: "plugin" }))).toBe("Plugin");
		expect(markLabel("shadowed")).toBe("A global skill of the same name runs instead");
		expect(markLabel("conflict")).toBe("Also in another folder, a different copy");
	});
});

describe("listing budget", () => {
	it("sizes the listing from the context window per CLI", () => {
		expect(listingLimit("claude", 200_000)).toBe(8_000);
		expect(listingLimit("codex", 200_000)).toBe(16_000);
		expect(listingLimit("claude", null)).toBe(FALLBACK_LISTING);
		expect(listingLimit("cursor", 200_000)).toBeNull();
		expect(listingLimit("acp", null)).toBeNull();
	});

	it("counts the name, a capped description and the line around each listed skill", () => {
		const long = "x".repeat(MAX_ENTRY_DESCRIPTION + 100);
		const budget = budgetOf(
			"claude",
			[
				skill({ name: "abc", description: "héllo" }),
				skill({ name: "long", description: long }),
				skill({ name: "off", enabled: false, description: "ignored" }),
				skill({ name: "mine", user_invocation_only: true }),
				skill({ name: "other", cli: "codex" }),
			],
			null
		);
		expect(budget).toEqual({
			cli: "claude",
			listed: 2,
			used: 3 + 5 + 5 + 4 + MAX_ENTRY_DESCRIPTION + 5,
			limit: FALLBACK_LISTING,
			over: false,
		});
	});

	it("goes over past the limit and warns only when an apply pushes it over", () => {
		const many = Array.from({ length: 10 }, (_, i) =>
			skill({ name: `skill-${i}`, description: "y".repeat(1_000) })
		);
		const small = budgetOf("claude", many.slice(0, 2), 100_000);
		const big = budgetOf("claude", many, 100_000);
		expect(small.over).toBe(false);
		expect(big.over).toBe(true);
		expect(pushesOut(small, big)).toBe(true);
		expect(pushesOut(big, big)).toBe(false);
		expect(budgetOf("cursor", many, 100_000).over).toBe(false);
	});
});
