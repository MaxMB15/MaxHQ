// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { SkillLibrary } from "../bindings";

import { EMPTY_SOURCE, originOf, planSource } from "./source-form";

const library: SkillLibrary = {
	device: "dev-mac",
	always: [],
	sources: [
		{
			id: "s1",
			name: "example/agent-skills",
			kind: { kind: "git", url: "https://github.com/example/agent-skills" },
			default_ref: null,
			trust: "own",
			commit: null,
			fetched_at: null,
			problem: null,
			skills: [],
		},
	],
};

describe("the new source form", () => {
	it("reads a folder, owner/repo and git URLs", () => {
		expect(originOf(" /Users/me/skills/ ")).toEqual({
			kind: "local",
			path: "/Users/me/skills",
		});
		expect(originOf("example/devflow")).toEqual({
			kind: "git",
			url: "https://github.com/example/devflow",
		});
		expect(originOf("git@github.com:example/devflow.git")).toEqual({
			kind: "git",
			url: "git@github.com:example/devflow.git",
		});
		expect(originOf("https://example.com/skills.git")?.kind).toBe("git");
		expect(originOf("skills")).toBeNull();
	});

	it("names a source after its repo when you don't, and keeps names apart", () => {
		expect(
			planSource(library, { ...EMPTY_SOURCE, where: "git@github.com:example/devflow.git" })
		).toEqual({
			ok: true,
			name: "example/devflow",
			origin: { kind: "git", url: "git@github.com:example/devflow.git" },
			ref: null,
			trust: "own",
		});
		expect(planSource(library, { ...EMPTY_SOURCE, where: "example/agent-skills" })).toEqual({
			ok: false,
			reason: "The library already has a source called example/agent-skills.",
		});
	});

	it("takes a ref and a third party's trust, but no ref for a folder", () => {
		expect(
			planSource(library, {
				name: "Theirs",
				where: "example/theirs",
				ref: " v1.2.0 ",
				thirdParty: true,
			})
		).toMatchObject({ ok: true, name: "Theirs", ref: "v1.2.0", trust: "third_party" });
		expect(planSource(library, { ...EMPTY_SOURCE, where: "/skills", ref: "main" })).toEqual({
			ok: false,
			reason: "A folder has no branch or tag to fetch.",
		});
		expect(planSource(library, EMPTY_SOURCE).ok).toBe(false);
	});
});
