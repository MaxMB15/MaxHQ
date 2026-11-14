// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentPlace, EffectiveSkill } from "../bindings";
import type { ActionResult } from "../data/source";
import { MAC } from "../data/mock-view";

import { clearSkillScans, rescanSkills, scanKey, skillScanOf } from "./scan";

const PLACE: AgentPlace = { type: "folder", path: "/work/repo" };

const SKILLS: EffectiveSkill[] = [
	{
		name: "git-flow",
		description: null,
		path: "/work/repo/.claude/skills/git-flow/SKILL.md",
		scope: null,
		enabled: true,
		display_name: null,
		short_description: null,
		user_invocation_only: null,
		user_invocable: null,
		place: "project",
		cli: "claude",
		mark: null,
	},
];

const answered: ActionResult = { status: "ok", data: { status: "skills", skills: SKILLS } };

afterEach(clearSkillScans);

describe("skill scans", () => {
	it("keys a place by device and kind", () => {
		expect(scanKey(MAC, PLACE)).not.toBe(
			scanKey(MAC, { type: "workspace", workspace: "/work/repo" })
		);
		expect(scanKey(MAC, PLACE)).not.toBe(scanKey("dev-other", PLACE));
	});

	it("shows scanning, then the skills found", async () => {
		const scanSkills = vi.fn(async () => answered);
		const running = rescanSkills({ scanSkills }, MAC, PLACE);
		expect(skillScanOf(MAC, PLACE)).toEqual({ state: "scanning", skills: null });
		await running;
		expect(skillScanOf(MAC, PLACE)).toEqual({ state: "done", skills: SKILLS });
		expect(scanSkills).toHaveBeenCalledWith(MAC, PLACE);
	});

	it("asks once while a scan runs", async () => {
		const scanSkills = vi.fn(async () => answered);
		await Promise.all([
			rescanSkills({ scanSkills }, MAC, PLACE),
			rescanSkills({ scanSkills }, MAC, PLACE),
		]);
		expect(scanSkills).toHaveBeenCalledTimes(1);
	});

	it("keeps the last skills when a scan fails", async () => {
		await rescanSkills({ scanSkills: async () => answered }, MAC, PLACE);
		await rescanSkills(
			{
				scanSkills: async () => ({
					status: "ok",
					data: { status: "refused", reason: "Skills aren't set up yet." },
				}),
			},
			MAC,
			PLACE
		);
		expect(skillScanOf(MAC, PLACE)).toMatchObject({ state: "failed", skills: SKILLS });
		expect((skillScanOf(MAC, PLACE) as { error: string }).error).toContain("set up");
	});
});
