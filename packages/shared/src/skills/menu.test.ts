// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { EffectiveSkill } from "../bindings";

import { dedupeSkillsByName, isUserInvocable, skillHint, skillLabel, skillMenu } from "./menu";

const skill = (over: Partial<EffectiveSkill> & { name: string }): EffectiveSkill => ({
	description: null,
	path: `/work/repo/.agents/skills/${over.name}/SKILL.md`,
	scope: null,
	enabled: true,
	display_name: null,
	short_description: null,
	user_invocation_only: null,
	user_invocable: null,
	place: "project",
	cli: "codex",
	mark: null,
	...over,
});

describe("the $ menu", () => {
	it("keeps each name once, ignoring case, the first found winning", () => {
		const first = skill({ name: "Git-Flow", place: "project" });
		const kept = dedupeSkillsByName([first, skill({ name: " git-flow ", place: "global" })]);
		expect(kept).toEqual([first]);
	});

	it("leaves out disabled and agent-only skills and other CLIs' skills", () => {
		expect(isUserInvocable(skill({ name: "a", enabled: false }))).toBe(false);
		expect(isUserInvocable(skill({ name: "a", user_invocable: false }))).toBe(false);
		expect(isUserInvocable(skill({ name: "a" }))).toBe(true);
		const menu = skillMenu(
			[
				skill({ name: "off", enabled: false }),
				skill({ name: "agent-only", user_invocable: false }),
				skill({ name: "claude-only", cli: "claude" }),
				skill({ name: "kept" }),
			],
			"codex",
			""
		);
		expect(menu.map((s) => s.name)).toEqual(["kept"]);
	});

	it("labels and hints a row", () => {
		expect(skillLabel(skill({ name: "grill-me" }))).toBe("Grill Me");
		expect(skillLabel(skill({ name: "x", display_name: " Grill " }))).toBe("Grill");
		expect(
			skillHint(skill({ name: "a", short_description: "Short", description: "Long" }))
		).toBe("Short");
		expect(skillHint(skill({ name: "a", description: "Long" }))).toBe("Long");
		expect(skillHint(skill({ name: "a", scope: "system" }))).toBe("system skill");
		expect(skillHint(skill({ name: "a" }))).toBe("Skill");
	});

	it("ranks the name over the label over the descriptions, dropping a leading $", () => {
		const menu = skillMenu(
			[
				skill({ name: "notes", description: "Plan the work" }),
				skill({ name: "release-plan" }),
				skill({ name: "plan" }),
				skill({ name: "planner" }),
				skill({ name: "other" }),
			],
			"codex",
			"$Plan"
		);
		expect(menu.map((s) => s.name)).toEqual(["plan", "planner", "release-plan", "notes"]);
	});
});
