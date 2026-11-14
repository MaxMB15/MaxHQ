// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { CheckoutSkills, SkillLibrary } from "../bindings";
import { DEVFLOW, PACK_TREE } from "../data/mock-skills";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import { skillLibrary } from "./library";
import { packHasUpdate, packSources, packStateText, withPack } from "./pack";
import { projectSkillsOn } from "./project";

const load = async (scenario?: string): Promise<ViewState> =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false, scenario }).snapshot());

const checkout = (pack: CheckoutSkills["pack"]): CheckoutSkills => ({
	path: "/work/repo",
	applied: true,
	drift: [],
	needs_restart: false,
	pack,
});

describe("the devflow pack", () => {
	it("offers only the sources with a pack", async () => {
		const library = skillLibrary(await load())!;
		expect(packSources(library).map((s) => s.id)).toEqual([DEVFLOW]);
	});

	it("picks, moves and takes out a lock's pack", async () => {
		const view = await load();
		const library = skillLibrary(view)!;
		const lock = projectSkillsOn(view, "proj-comet", MAC)!.lock;
		expect(lock.pack).toBeUndefined();

		const picked = withPack(library, lock, DEVFLOW);
		expect(picked.pack).toEqual({ source: DEVFLOW, commit: "8be68f9", tree: PACK_TREE });
		expect(picked.skills).toEqual(lock.skills);
		expect(packHasUpdate(library, picked)).toBe(false);

		const moved: SkillLibrary = {
			...library,
			sources: library.sources.map((s) => (s.id === DEVFLOW ? { ...s, pack: "d00d" } : s)),
		};
		expect(packHasUpdate(moved, picked)).toBe(true);
		expect(withPack(moved, picked, DEVFLOW).pack?.tree).toBe("d00d");

		expect("pack" in withPack(library, picked, null)).toBe(false);
		// A source without a pack can't be picked.
		expect(withPack(library, lock, library.sources[0]!.id).pack).toBeUndefined();
	});

	it("says where each checkout's pack stands", async () => {
		expect(packStateText(checkout(null))).toBeNull();
		expect(packStateText(checkout({ state: "installed", tree: PACK_TREE }))).toBe(
			"Installed at c41e9b0"
		);
		expect(packStateText(checkout({ state: "installing", tree: PACK_TREE }))).toBe(
			"Installing c41e9b0"
		);
		expect(packStateText(checkout({ state: "failed", tree: PACK_TREE, lines: [] }))).toBe(
			"Install of c41e9b0 failed"
		);
		expect(packStateText(checkout({ state: "vendored" }))).toMatch(/^Vendored by someone else/);
	});

	it("shows a failed install on the devbox under drift until an apply", async () => {
		const view = await load("skills_drift");
		const devbox = projectSkillsOn(view, "proj-maxhq", DEVBOX)!.checkouts[0]!;
		expect(devbox.pack?.state).toBe("failed");
		const comet = projectSkillsOn(view, "proj-comet", MAC)!.checkouts[0]!;
		expect(comet.pack).toEqual({ state: "vendored" });
	});
});
