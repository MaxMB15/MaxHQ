// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { EffectiveSkill, ProjectSkills, SkillLibrary, SkillLock } from "../bindings";
import { NOW } from "../test/build";
import {
	AGENT_SKILLS,
	GIT_FLOW_PROPOSAL,
	isSkillScenario,
	mockSkills,
	noSkills,
	SKILL_SCENARIOS,
	THIRD_PARTY,
} from "./mock-skills";
import { DEVBOX, MAC } from "./mock-view";

const DONE = { status: "ok", data: { status: "done" } };

const setup = (at = "connected") => {
	let scenario = at;
	const changed = vi.fn();
	const mock = mockSkills({ now: () => NOW, scenario: () => scenario, changed });
	const library = () =>
		mock
			.entities(scenario)
			.find((e) => e.entity === "skill_library") as unknown as SkillLibrary;
	const project = (id: string, device = MAC) =>
		mock
			.entities(scenario)
			.find(
				(e) => e.entity === "project_skills" && e.project === id && e.device === device
			) as unknown as ProjectSkills | undefined;
	const scan = async (path: string, device = MAC): Promise<EffectiveSkill[]> => {
		const answer = await mock.calls.scanSkills(device, { type: "folder", path });
		if (answer.status !== "ok" || answer.data.status !== "skills") throw new Error("no scan");
		return answer.data.skills;
	};
	return {
		...mock,
		changed,
		library,
		project,
		scan,
		show: (next: string) => (scenario = next),
	};
};

const MAXHQ = "/Users/me/Code/Github/Multi/MaxAIManager";
const COMET = "/Users/me/Code/Github/Multi/Comet";

describe("noSkills", () => {
	it("refuses every call in words", async () => {
		const refused = {
			status: "ok",
			data: { status: "refused", reason: "Skills aren't set up yet." },
		};
		expect(await noSkills.scanSkills(MAC, { type: "folder", path: "/work/repo" })).toEqual(
			refused
		);
		expect(await noSkills.answerSkillProposal(GIT_FLOW_PROPOSAL, "reject", false)).toEqual(
			refused
		);
	});
});

describe("mockSkills", () => {
	it("names its scenarios", () => {
		expect(SKILL_SCENARIOS.map((s) => s.id)).toEqual([
			"skills_conflict",
			"skills_proposal",
			"skills_drift",
		]);
		expect(isSkillScenario("skills_drift")).toBe(true);
		expect(isSkillScenario("connected")).toBe(false);
	});

	it("holds the library on the Mac and locks MaxHQ on both devices", () => {
		const { library, project } = setup();
		expect(library().device).toBe(MAC);
		expect(library().always).toEqual(["unslop", "propose-skill-change"]);
		expect(project("proj-maxhq")?.lock.skills.map((s) => [s.name, s.pinned])).toEqual([
			["git-flow", false],
			["screen-reader-check", true],
		]);
		expect(project("proj-maxhq", DEVBOX)).toBeDefined();
	});

	it("shows the proposal only in its scenario and rejects it", async () => {
		const { entities, calls, show, changed } = setup();
		expect(entities("connected").some((e) => e.entity === "skill_proposal")).toBe(false);
		show("skills_proposal");
		expect(entities("skills_proposal").some((e) => e.entity === "skill_proposal")).toBe(true);
		expect(await calls.answerSkillProposal(GIT_FLOW_PROPOSAL, "reject", false)).toEqual(DONE);
		expect(changed).toHaveBeenCalled();
		expect(entities("skills_proposal").some((e) => e.entity === "skill_proposal")).toBe(false);
		expect(await calls.answerSkillProposal(GIT_FLOW_PROPOSAL, "reject", false)).toMatchObject({
			status: "error",
		});
	});

	it("sends the proposal's diff and approves it", async () => {
		const { entities, calls, show } = setup();
		show("skills_proposal");
		const read = await calls.skillProposalDiff(GIT_FLOW_PROPOSAL);
		expect(read).toMatchObject({ status: "ok", data: { status: "skill_proposal_diff" } });
		const diff =
			read.status === "ok" && read.data.status === "skill_proposal_diff"
				? read.data.diff
				: null;
		expect(diff?.files.map((f) => [f.path, f.status])).toEqual([
			["SKILL.md", "modified"],
			["references/release.md", "added"],
		]);
		expect(await calls.answerSkillProposal(GIT_FLOW_PROPOSAL, "approve", true)).toEqual(DONE);
		expect(entities("skills_proposal").some((e) => e.entity === "skill_proposal")).toBe(false);
		expect(await calls.skillProposalDiff(GIT_FLOW_PROPOSAL)).toMatchObject({ status: "error" });
	});

	it("marks git-flow shadowed for Claude, the global copies the same and devflow vendored", async () => {
		const { scan } = setup();
		const maxhq = await scan(MAXHQ);
		const mark = (skills: EffectiveSkill[], cli: string, name: string, place: string) =>
			skills.find((s) => s.cli === cli && s.name === name && s.place === place)?.mark;
		expect(mark(maxhq, "claude", "git-flow", "project")).toBe("shadowed");
		expect(mark(maxhq, "codex", "git-flow", "project")).toBeNull();
		expect(mark(maxhq, "codex", "git-flow", "global")).toBe("same");
		expect(maxhq.find((s) => s.scope === "synced")?.name).toBe("brand-voice");
		const comet = await scan(COMET);
		expect(comet.filter((s) => s.mark === "vendored").map((s) => s.name)).toEqual([
			"df-plan",
			"unslop",
		]);
	});

	it("shows unslop in conflict until the global tidy, and undoes it", async () => {
		const { calls, scan } = setup("skills_conflict");
		const conflicts = async () =>
			(await scan(MAXHQ)).filter((s) => s.mark === "conflict").length;
		expect(await conflicts()).toBeGreaterThan(0);
		const plan = await calls.planGlobalSkills(MAC);
		if (plan.status !== "ok" || plan.data.status !== "skills_globals_plan") {
			throw new Error("no plan");
		}
		const { hash, joins, conflicts: differ, undo } = plan.data.plan;
		expect(undo).toBe(false);
		expect(joins?.map((j) => j.name)).toEqual(["owner/skills"]);
		expect(differ?.map((c) => c.name)).toEqual(["review"]);
		expect((await calls.applyGlobalSkills(MAC, "stale", [])).status).toBe("ok");
		expect(await calls.applyGlobalSkills(MAC, hash, ["/nowhere"])).toMatchObject({
			data: { status: "refused", reason: "/nowhere isn't a copy you can pick." },
		});
		expect(await calls.applyGlobalSkills(MAC, hash, [differ![0]!.paths[0]!])).toEqual(DONE);
		expect(await conflicts()).toBe(0);
		expect(await calls.planGlobalSkills(MAC)).toMatchObject({
			data: { plan: { undo: true } },
		});
		expect(await calls.undoGlobalSkills(MAC)).toEqual(DONE);
		expect(await conflicts()).toBeGreaterThan(0);
		expect(await calls.planGlobalSkills(DEVBOX)).toMatchObject({
			data: { status: "refused" },
		});
	});

	it("adds, fetches and removes sources on the holder only", async () => {
		const { calls, library } = setup();
		const origin = { kind: "git" as const, url: "https://example.com/owner/repo.git" };
		expect(await calls.addSkillSource(DEVBOX, "more", origin, "main", "own")).toMatchObject({
			data: { status: "refused" },
		});
		expect(await calls.addSkillSource(MAC, "more", origin, "main", "own")).toEqual(DONE);
		expect(await calls.addSkillSource(MAC, "more", origin, "main", "own")).toMatchObject({
			data: { status: "refused" },
		});
		const added = library().sources.at(-1)!;
		expect(added).toMatchObject({ name: "more", kind: origin, skills: [] });
		expect(await calls.fetchSkillSource(MAC, added.id)).toEqual(DONE);
		expect(await calls.removeSkillSource(MAC, THIRD_PARTY)).toMatchObject({
			data: { status: "refused" },
		});
		expect(await calls.removeSkillSource(MAC, added.id)).toEqual(DONE);
		expect(library().sources.some((s) => s.id === added.id)).toBe(false);
	});

	it("moves the library with its sources and pushes on the holder only", async () => {
		const { calls, changed, library } = setup();
		const ids = library().sources.map((s) => s.id);
		expect(await calls.pushSkillSource(DEVBOX, AGENT_SKILLS)).toMatchObject({
			data: { status: "refused" },
		});
		expect(await calls.pushSkillSource(MAC, AGENT_SKILLS)).toEqual(DONE);
		expect(await calls.moveSkillLibrary(DEVBOX)).toEqual(DONE);
		expect(library().device).toBe(DEVBOX);
		expect(library().sources.map((s) => s.id)).toEqual(ids);
		expect(changed).toHaveBeenCalled();
		expect(await calls.moveSkillLibrary(DEVBOX)).toEqual(DONE);
	});

	it("refuses an always skill in a lock and applies a new lock", async () => {
		const { calls, project } = setup();
		const lock = project("proj-comet")!.lock;
		const always: SkillLock = {
			...lock,
			skills: [{ ...lock.skills[0]!, name: "unslop", source: AGENT_SKILLS }],
		};
		expect(await calls.setProjectSkills(MAC, "proj-comet", always)).toMatchObject({
			data: { status: "refused", reason: "Every agent already gets unslop." },
		});
		expect(await calls.setProjectSkills(MAC, "proj-comet", { ...lock, skills: [] })).toEqual(
			DONE
		);
		expect(project("proj-comet")?.checkouts.every((c) => !c.applied)).toBe(true);
		expect(await calls.applySkills(MAC, "proj-comet")).toEqual(DONE);
		expect(project("proj-comet")?.checkouts.every((c) => c.applied)).toBe(true);
		expect(await calls.applySkills(DEVBOX, "proj-comet")).toMatchObject({
			data: { status: "refused" },
		});
	});

	it("puts drift on Comet in the drift scenario", () => {
		const { project, show } = setup();
		expect(project("proj-comet")?.checkouts[0]?.drift).toEqual([]);
		show("skills_drift");
		expect(project("proj-comet")?.checkouts[0]).toMatchObject({
			drift: ["grill-me"],
			needs_restart: true,
		});
	});

	it("clears the drift once Restore applies the lock again", async () => {
		const { calls, project } = setup("skills_drift");
		expect(project("proj-comet")?.checkouts[0]?.drift).toEqual(["grill-me"]);
		expect(await calls.applySkills(MAC, "proj-comet")).toEqual(DONE);
		expect(project("proj-comet")?.checkouts[0]).toMatchObject({
			drift: [],
			needs_restart: true,
		});
	});
});
