// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { EffectiveSkill, SkillLibrary, SkillProposal } from "../bindings";
import { GIT_FLOW_PROPOSAL, THIRD_PARTY } from "../data/mock-skills";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import { skillLibrary, skillProposals } from "./library";
import {
	afterApply,
	budgetsOf,
	cantApprove,
	devicesWithSkills,
	EMPTY_LOCK,
	hasUpdates,
	hideHint,
	needsApply,
	otherLocks,
	pinSkill,
	projectSkillsOn,
	proposalFrom,
	proposalTitle,
	pushedOut,
	scanPlaceOf,
	skillDevices,
	updatedLock,
	updateOf,
	windowOf,
	withoutSkill,
} from "./project";

const load = async (scenario?: string): Promise<ViewState> =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false, scenario }).snapshot());

const libraryOf = (view: ViewState): SkillLibrary => skillLibrary(view)!;

const effective = (over: Partial<EffectiveSkill> & { name: string }): EffectiveSkill => ({
	description: null,
	path: `/w/${over.name}/SKILL.md`,
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

describe("the project's skills", () => {
	it("lists the devices that keep or could keep them, and where each scans", async () => {
		const view = await load();
		expect(skillDevices(view, "proj-maxhq")).toEqual([DEVBOX, MAC].sort());
		expect(scanPlaceOf(view, "proj-maxhq", MAC)).toEqual({
			type: "workspace",
			workspace: "ws-mac-maxhq",
		});
		expect(scanPlaceOf(view, "proj-none", MAC)).toBeNull();
		expect(projectSkillsOn(view, "proj-comet", MAC)?.lock.committed).toBe(true);
	});

	it("says when a checkout lacks the lock's skills", async () => {
		const view = await load();
		const skills = projectSkillsOn(view, "proj-maxhq", MAC)!;
		expect(needsApply(skills)).toBe(false);
		expect(
			needsApply({ ...skills, checkouts: [{ ...skills.checkouts[0]!, applied: false }] })
		).toBe(true);
		expect(needsApply(undefined)).toBe(false);
	});

	it("takes a skill out and pins one", async () => {
		const lock = projectSkillsOn(await load(), "proj-maxhq", MAC)!.lock;
		expect(withoutSkill(lock, "git-flow").skills.map((s) => s.name)).toEqual([
			"screen-reader-check",
		]);
		expect(pinSkill(lock, "git-flow", true).skills[0]!.pinned).toBe(true);
		expect(EMPTY_LOCK.skills).toEqual([]);
	});

	it("moves unpinned skills to their source's newest copy and leaves pinned ones", async () => {
		const view = await load();
		const library = libraryOf(view);
		const lock = projectSkillsOn(view, "proj-maxhq", MAC)!.lock;
		expect(hasUpdates(library, lock)).toBe(false);
		const old = {
			...lock,
			skills: lock.skills.map((s) => ({ ...s, tree: "tree-old", commit: "0000000" })),
		};
		expect(hasUpdates(library, old)).toBe(true);
		expect(updateOf(library, old.skills[0]!)).toMatchObject({
			name: "git-flow",
			tree: "tree-git-flow",
			commit: "4f1c2a9",
		});
		// The third party's skill is pinned, so it stays put.
		expect(updateOf(library, old.skills[1]!)).toBeNull();
		expect(updatedLock(library, old).skills.map((s) => s.tree)).toEqual([
			"tree-git-flow",
			"tree-old",
		]);
	});

	it("sizes each CLI's listing by the project's agents' biggest window", async () => {
		const view = await load();
		const claude = windowOf(view, "proj-maxhq", "claude");
		expect(claude).toBeGreaterThan(0);
		expect(windowOf(view, "proj-maxhq", "acp")).toBeNull();
		const budgets = budgetsOf(view, "proj-maxhq", [effective({ name: "a" })]);
		expect(budgets).toEqual([
			expect.objectContaining({ cli: "claude", listed: 1, limit: (claude! * 4) / 100 }),
		]);
	});

	it("swaps the project's own skills for the lock's, keeping vendored ones", async () => {
		const view = await load();
		const library = libraryOf(view);
		const lock = projectSkillsOn(view, "proj-maxhq", MAC)!.lock;
		const after = afterApply(library, lock, [
			effective({ name: "old" }),
			effective({ name: "df-plan", mark: "vendored" }),
			effective({ name: "brand-voice", place: "global" }),
			effective({ name: "plan", cli: "codex", place: "system" }),
		]);
		expect(after.map((s) => `${s.cli}:${s.name}`)).toEqual([
			"claude:df-plan",
			"claude:brand-voice",
			"codex:plan",
			"claude:git-flow",
			"claude:screen-reader-check",
			"codex:git-flow",
			"codex:screen-reader-check",
		]);
		expect(after.find((s) => s.name === "git-flow")?.description).toContain("Branch");
	});

	it("warns only for a CLI an apply pushes past its listing", async () => {
		const view = await load();
		const library = libraryOf(view);
		const long = "x".repeat(1500);
		const lock = {
			skills: library.sources.flatMap((source) =>
				source.skills.map((skill) => ({
					name: skill.name,
					source: source.id,
					path: skill.path,
					commit: source.commit,
					tree: skill.tree,
					pinned: false,
				}))
			),
			committed: false,
		};
		const wide = {
			...library,
			sources: library.sources.map((source) => ({
				...source,
				skills: source.skills.map((skill) => ({ ...skill, description: long })),
			})),
		};
		const before = [effective({ name: "a" }), effective({ name: "b", cli: "cursor" })];
		expect(pushedOut(view, "proj-acp", wide, lock, before)).toEqual(["claude"]);
		expect(pushedOut(view, "proj-acp", library, EMPTY_LOCK, before)).toEqual([]);
	});

	it("names a proposal's agent and refuses a third party's skill", async () => {
		const view = await load("skills_proposal");
		const [proposal] = skillProposals(view, "proj-maxhq");
		expect(proposal!.id).toBe(GIT_FLOW_PROPOSAL);
		expect(proposalFrom(view, proposal!)).toBe(
			view.tables.agent.get("agent-release")?.title ?? "an agent"
		);
		const nobody: SkillProposal = { ...proposal!, agent: null };
		expect(proposalFrom(view, nobody)).toBe("an agent");
		expect(proposalTitle(view, proposal!)).toBe(
			`Skill change from ${proposalFrom(view, proposal!)}`
		);
		expect(proposalTitle(view, nobody)).toBe(
			`Skill change in ${view.tables.project.get("proj-maxhq")!.name}`
		);
		expect(cantApprove(libraryOf(view), proposal!)).toBeNull();
		const theirs: SkillProposal = { ...proposal!, source: THIRD_PARTY };
		expect(cantApprove(libraryOf(view), theirs)).toMatch(/third party's\. Name a fork/);
		expect(cantApprove(libraryOf(view), theirs, "  ")).toMatch(/Name a fork/);
		expect(cantApprove(libraryOf(view), theirs, "git-flow")).toMatch(/name of its own/);
		expect(cantApprove(libraryOf(view), theirs, "my-git-flow")).toBeNull();
	});
});

describe("the library page's devices", () => {
	it("lists each device that takes skills requests and says how to hide a CLI's skill", async () => {
		const view = await load();
		expect(devicesWithSkills(view)).toEqual(expect.arrayContaining([MAC, DEVBOX]));
		expect(hideHint("codex")).toContain("config.toml");
		expect(hideHint("claude")).toContain("skillOverrides");
		expect(hideHint("cursor")).toBeNull();
	});
});

describe("adding from another project", () => {
	it("offers the other projects with skills on the same device", async () => {
		const view = await load();
		expect(otherLocks(view, "proj-maxhq", MAC).map((p) => p.project)).toEqual(["proj-comet"]);
		expect(otherLocks(view, "proj-maxhq", DEVBOX)).toEqual([]);
	});
});
