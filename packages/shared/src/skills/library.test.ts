// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, SkillLibrary } from "../bindings";
import { AGENT_SKILLS, GIT_FLOW_PROPOSAL, THIRD_PARTY } from "../data/mock-skills";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import {
	addFromProject,
	badgeLabel,
	cantLock,
	driftOf,
	outsideOf,
	lockedFrom,
	offersSkills,
	projectSkillsOf,
	searchLibrary,
	skillBadges,
	skillLibrary,
	skillProposals,
	sourceWhere,
	waitsForSkills,
} from "./library";

const load = async (scenario?: string): Promise<ViewState> =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false, scenario }).snapshot());

const libraryOf = (view: ViewState): SkillLibrary => {
	const library = skillLibrary(view);
	if (!library) throw new Error("no library");
	return library;
};

const withEntities = (view: ViewState, extra: Entity[]): ViewState =>
	applySnapshot({
		seq: 1,
		entities: [
			...Object.values(view.tables).flatMap((table) => [...table.values()] as Entity[]),
			...extra,
		],
	});

describe("skills selectors", () => {
	it("says which devices take skills requests", async () => {
		const view = await load();
		expect(offersSkills(view, MAC)).toBe(true);
		expect(offersSkills(view, "dev-gone")).toBe(false);
	});

	it("shows this Mac's library over one a device that sorts first says it holds", async () => {
		const view = await load();
		const planted: Entity = { entity: "skill_library", ...libraryOf(view), device: DEVBOX };
		expect(DEVBOX < MAC).toBe(true);
		expect(libraryOf(withEntities(view, [planted])).device).toBe(MAC);
	});

	it("finds the library on the Mac with its three sources", async () => {
		const library = libraryOf(await load());
		expect(library.device).toBe(MAC);
		expect(library.sources.map((s) => s.trust)).toEqual(["own", "own", "third_party"]);
		expect(sourceWhere(library.sources[0]!.kind)).toContain("example/agent-skills");
	});

	it("prefers the connected holder while two devices publish a library", async () => {
		const view = await load("peer_offline");
		const mine = libraryOf(view);
		const moved = withEntities(view, [
			{ entity: "skill_library", ...mine, device: DEVBOX, sources: [] },
		]);
		expect(libraryOf(moved).device).toBe(MAC);
	});

	it("badges what a skill does beyond its words", async () => {
		const library = libraryOf(await load());
		const third = library.sources.find((s) => s.id === THIRD_PARTY)!;
		const badges = skillBadges(third.skills[0]!, third);
		expect(badges).toEqual(["scripts", "hooks", "third_party"]);
		expect(badges.map(badgeLabel)).toEqual(["Scripts", "Hooks", "Third party"]);
		const own = library.sources.find((s) => s.id === AGENT_SKILLS)!;
		const native = own.skills.find((s) => s.name === "native-rendering-discipline")!;
		expect(skillBadges(native, own)).toEqual(["allowed_tools"]);
	});

	it("searches the library by every word, by name", async () => {
		const library = libraryOf(await load());
		expect(searchLibrary(library, "").length).toBe(
			library.sources.reduce((n, s) => n + s.skills.length, 0)
		);
		const names = searchLibrary(library, "").map((s) => s.skill.name);
		expect(names).toEqual([...names].sort((a, b) => a.localeCompare(b)));
		expect(searchLibrary(library, "GIT flow").map((s) => s.skill.name)).toEqual(["git-flow"]);
		expect(searchLibrary(library, "no such words")).toEqual([]);
	});

	it("pins a third party's skill when a lock takes it", async () => {
		const library = libraryOf(await load());
		const [third] = searchLibrary(library, "screen-reader-check");
		expect(lockedFrom(third!)).toMatchObject({ source: THIRD_PARTY, pinned: true });
		const [own] = searchLibrary(library, "grill-me");
		expect(lockedFrom(own!).pinned).toBe(false);
	});

	it("keeps the always set and names a project has out of its lock", async () => {
		const view = await load();
		const library = libraryOf(view);
		const [maxhq] = projectSkillsOf(view, "proj-maxhq");
		const [comet] = projectSkillsOf(view, "proj-comet");
		expect(cantLock(library, maxhq!.lock, "unslop")).toBe("Every agent already gets unslop");
		expect(cantLock(library, maxhq!.lock, "git-flow")).toBe("The project already has git-flow");
		expect(cantLock(library, maxhq!.lock, "grill-me")).toBeNull();
		const added = addFromProject(library, maxhq!.lock, comet!.lock);
		expect(added.skills.map((s) => s.name)).toEqual([
			...maxhq!.lock.skills.map((s) => s.name),
			"grill-me",
		]);
		expect(addFromProject(library, added, comet!.lock)).toEqual(added);
	});

	it("lists a project's skills per device", async () => {
		const view = await load();
		expect(projectSkillsOf(view, "proj-maxhq").map((p) => p.device)).toEqual(
			[DEVBOX, MAC].sort()
		);
		expect(projectSkillsOf(view, "proj-none")).toEqual([]);
	});

	it("shows drift and a waiting Cursor agent only in the drift scenario", async () => {
		const calm = await load();
		expect(driftOf(calm, "proj-comet")).toEqual([]);
		expect(waitsForSkills(calm, "agent-cursor-seek")).toBe(false);
		const drifted = await load("skills_drift");
		expect(driftOf(drifted, "proj-comet").map((d) => [d.device, d.checkout.drift])).toEqual([
			[MAC, ["grill-me"]],
		]);
		expect(waitsForSkills(drifted, "agent-cursor-seek")).toBe(true);
		expect(waitsForSkills(drifted, "agent-gone")).toBe(false);
	});

	it("names the links that stopped skills landing", async () => {
		expect(outsideOf(await load(), "proj-comet")).toEqual([]);
		const drifted = await load("skills_drift");
		expect(outsideOf(drifted, "proj-comet").map((o) => [o.device, o.checkout.outside])).toEqual(
			[[MAC, [".claude"]]]
		);
	});

	it("lists proposals newest first, by project", async () => {
		expect(skillProposals(await load())).toEqual([]);
		const view = await load("skills_proposal");
		const [found] = skillProposals(view);
		expect(found?.id).toBe(GIT_FLOW_PROPOSAL);
		const newer = withEntities(view, [
			{ entity: "skill_proposal", ...found!, id: "proposal-newer", created_at: NOW },
		]);
		expect(skillProposals(newer).map((p) => p.id)).toEqual([
			"proposal-newer",
			GIT_FLOW_PROPOSAL,
		]);
		expect(skillProposals(newer, "proj-maxhq")).toHaveLength(2);
		expect(skillProposals(newer, "proj-comet")).toEqual([]);
	});
});
