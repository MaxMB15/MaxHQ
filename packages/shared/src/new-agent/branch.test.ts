// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { setLanguageChoice } from "../i18n";
import { agent, entities, MIN, NOW, project, workspace } from "../test/build";
import { pullLink, snapshot, summary } from "../test/pulls";
import { applySnapshot, type ViewState } from "../view/replica";

import {
	agentsOnBranch,
	folderClash,
	branchProblem,
	branchSuggestions,
	branchTag,
	type BranchSuggestion,
	filterBranches,
} from "./branch";

// The names the guest's `check_name` is tested on too.
const NAMES = JSON.parse(
	readFileSync(
		new URL("../../../../crates/daemon/fixtures/branch-names.json", import.meta.url),
		"utf8"
	)
) as { good: string[]; bad: string[] };

describe("the name check", () => {
	it("takes the names the guest takes", () => {
		for (const name of NAMES.good) expect(branchProblem(name), name).toBeNull();
	});

	it("refuses the names the guest refuses", () => {
		for (const name of NAMES.bad) expect(branchProblem(name), name).not.toBeNull();
	});

	it("says what's wrong", () => {
		expect(branchProblem("")).toBe("Give the branch a name.");
		expect(branchProblem("-b")).toBe("A branch can't start with a dash.");
		expect(branchProblem("/a")).toBe("A branch can't start with a slash.");
		expect(branchProblem("a/")).toBe("A branch can't end with a slash.");
		expect(branchProblem("a.lock")).toBe("A branch can't end with .lock.");
		expect(branchProblem("a.")).toBe("A branch can't end with a dot.");
		expect(branchProblem("a..b")).toBe("A branch can't have two dots in a row.");
		expect(branchProblem("a//b")).toBe("A branch can't have two slashes in a row.");
		expect(branchProblem("a@{1}")).toBe('A branch can\'t have "@{" in it.');
		expect(branchProblem("fix login")).toBe("A branch can't have spaces.");
		expect(branchProblem("main~1")).toBe('A branch can\'t have "~" in it.');
		expect(branchProblem("café")).toBe('A branch can\'t have "é" in it.');
	});
});

// Project p1 in owner/repo, with a container and a Mac checkout, and p2.
const viewOf = (agents: Parameters<typeof agent>[0][] = []): ViewState =>
	applySnapshot({
		seq: 1,
		entities: entities({
			projects: [
				{ ...project("p1", "MaxHQ"), repo: "https://github.com/owner/repo.git" },
				project("p2", "Comet"),
			],
			workspaces: [
				workspace({
					id: "ws-box",
					project: "p1",
					location: { type: "container", instance: "box", path: "/work/repo" },
				}),
				workspace({ id: "ws-mac", project: "p1" }),
				workspace({ id: "ws-other", project: "p2" }),
			],
			agents: agents.map(agent),
		}),
	});

const names = (list: readonly BranchSuggestion[]) => list.map((s) => s.name);

describe("the suggestions", () => {
	it("put the branches agents work on first, newest first, then PR heads, then the repo's", () => {
		const view = viewOf([
			{ id: "old", workspace: "ws-box", branch: "fix/old", last_activity_at: NOW - 30 * MIN },
			{ id: "new", workspace: "ws-mac", branch: "fix/new", last_activity_at: NOW - MIN },
		]);
		const pulls = [
			summary(7, { head: "pr/older", updated_at: NOW - 20 * MIN }),
			summary(8, { head: "pr/newer", updated_at: NOW - 2 * MIN }),
		];
		const repo = { branches: ["main", "zeta", "pr/older"], default_branch: "main" };
		expect(branchSuggestions(view, "p1", pulls, repo)).toEqual([
			{ name: "fix/new", pull: null, current: true },
			{ name: "fix/old", pull: null, current: true },
			{ name: "pr/newer", pull: 8, current: false },
			{ name: "pr/older", pull: 7, current: false },
			{ name: "zeta", pull: null, current: false },
		]);
	});

	it("show a name once, with its PR and that an agent works on it", () => {
		const view = viewOf([{ id: "a1", workspace: "ws-box", branch: "topic" }]);
		const list = branchSuggestions(view, "p1", [summary(3, { head: "topic" })], {
			branches: ["topic"],
			default_branch: "main",
		});
		expect(list).toEqual([{ name: "topic", pull: 3, current: true }]);
		expect(branchTag(list[0]!)).toBe("PR #3");
		expect(branchTag({ name: "x", pull: null, current: true })).toBe("In use");
		expect(branchTag({ name: "x", pull: null, current: false })).toBeNull();
	});

	it("keeps PR numbers ungrouped in every interface language", () => {
		try {
			for (const language of ["en", "nl", "de", "fr", "es"] as const) {
				setLanguageChoice(language);
				expect(branchTag({ name: "topic", pull: 1234, current: false })).toMatch(/1234$/);
			}
		} finally {
			setLanguageChoice("en");
		}
	});

	it("take open PR heads from the PRs agents link to, in the project's repo", () => {
		const open = { ...pullLink(4), snapshot: snapshot({ head: "linked" }) };
		const closed = { ...pullLink(5), snapshot: snapshot({ head: "shut", state: "closed" }) };
		const elsewhere = {
			...pullLink(6),
			key: { repo: "someone/else", number: 6 },
			snapshot: snapshot({ head: "theirs" }),
		};
		const view = viewOf([
			{ id: "a1", workspace: "ws-mac", pulls: [open, closed, elsewhere, pullLink(9)] },
		]);
		expect(branchSuggestions(view, "p1", [], undefined)).toEqual([
			{ name: "linked", pull: 4, current: false },
		]);
	});

	it("leave out other projects' agents, sub-agents and closed PRs", () => {
		const view = viewOf([
			{ id: "other", workspace: "ws-other", branch: "comet" },
			{ id: "sub", workspace: "ws-box", branch: "child", parent: "other" },
		]);
		const pulls = [summary(1, { head: "merged", state: "merged" })];
		expect(branchSuggestions(view, "p1", pulls, undefined)).toEqual([]);
	});

	it("leave out the default branch and names the guest would refuse", () => {
		const repo = { branches: ["main", "dev", "feat+x", "release~1"], default_branch: "main" };
		expect(names(branchSuggestions(viewOf(), "p1", [], repo))).toEqual(["dev"]);
	});

	it("work with nothing to suggest", () => {
		expect(branchSuggestions(viewOf(), "p1", [], undefined)).toEqual([]);
	});
});

describe("filtering", () => {
	const all = ["fix/login-page", "Feature/Login", "docs"].map((name): BranchSuggestion => ({
		name,
		pull: null,
		current: false,
	}));

	it("keeps them all when nothing's typed", () => {
		expect(names(filterBranches(all, "  "))).toEqual([
			"fix/login-page",
			"Feature/Login",
			"docs",
		]);
	});

	it("keeps the names with what's typed in them, ignoring case, in order", () => {
		expect(names(filterBranches(all, "LOGIN"))).toEqual(["fix/login-page", "Feature/Login"]);
		expect(names(filterBranches(all, "nothing"))).toEqual([]);
	});

	it("matches spaces typed where a branch has dashes", () => {
		expect(names(filterBranches(all, "login page"))).toEqual(["fix/login-page"]);
	});
});

describe("who works on a branch", () => {
	it("finds the other agents in the workspace on it", () => {
		const view = viewOf([
			{ id: "same", workspace: "ws-box", branch: "fix" },
			{ id: "mac", workspace: "ws-mac", branch: "fix" },
			{ id: "else", workspace: "ws-box", branch: "other" },
			{ id: "sub", workspace: "ws-box", branch: "fix", parent: "same" },
		]);
		expect(agentsOnBranch(view, "ws-box", "fix").map((a) => a.id)).toEqual(["same"]);
		expect(agentsOnBranch(view, "ws-box", "new")).toEqual([]);
	});

	it("says when another agent's branch has the same folder", () => {
		const view = viewOf([
			{ id: "slash", workspace: "ws-box", branch: "fix/login" },
			{ id: "mac", workspace: "ws-mac", branch: "fix-page" },
		]);
		expect(folderClash(view, "ws-box", "fix-login")).toBe(
			"fix/login already has this branch's folder. Pick another name."
		);
		expect(folderClash(view, "ws-box", "fix/login")).toBeNull();
		expect(folderClash(view, "ws-box", "fix/page")).toBeNull();
	});
});
