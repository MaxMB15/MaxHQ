// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";
import { i18n, setLanguageChoice } from "../i18n";

import type { PullSnapshot } from "../bindings";
import { pullLink, REPO, snapshot } from "../test/pulls";

import { chipChecks, chipLabel, chipState, foldChips, pullPrompts } from "./chip";
import { pullUrl } from "./urls";

const read = (over: Partial<PullSnapshot> = {}) => ({
	...pullLink(7),
	snapshot: snapshot(over),
});

describe("chipState and chipChecks", () => {
	it("show GitHub's state, with a draft apart and neutral before the first read", () => {
		expect(chipState(pullLink(7))).toBe("unread");
		expect(chipState(read())).toBe("open");
		expect(chipState(read({ draft: true }))).toBe("draft");
		expect(chipState(read({ state: "merged" }))).toBe("merged");
		expect(chipState(read({ state: "closed", draft: true }))).toBe("closed");
	});

	it("show no dot for a PR with no checks or not read yet", () => {
		expect(chipChecks(pullLink(7))).toBeNull();
		expect(chipChecks(read({ checks: "none" }))).toBeNull();
		expect(chipChecks(read({ checks: "failing" }))).toBe("failing");
		expect(chipChecks(read({ checks: "unknown" }))).toBe("unknown");
	});
});

describe("chipLabel", () => {
	it("names the PR, its title, its state and its checks", () => {
		expect(chipLabel(read({ checks: "pending" }))).toBe(
			"Pull request 7, A change, open, checks running"
		);
		expect(chipLabel(pullLink(7))).toBe("Pull request 7, not read yet");
	});
});

describe("foldChips", () => {
	it("counts the PRs, keeps a state they share and shows the checks that need you most", () => {
		const folded = foldChips([
			{ ...read({ checks: "passing" }), key: { repo: REPO, number: 3 } },
			read({ checks: "failing", title: "Another" }),
		]);
		expect(folded).toEqual({
			text: "2 PRs",
			state: "open",
			checks: "failing",
			title: "#3 A change\n#7 Another",
			label: "2 pull requests: Pull request 3, A change, open, checks passing; Pull request 7, Another, open, checks failing",
		});
	});

	it("shows no state when they differ, and no checks when none has any", () => {
		const folded = foldChips([pullLink(3), read({ state: "merged", checks: "none" })]);
		expect(folded.state).toBeNull();
		expect(folded.checks).toBeNull();
		expect(folded.title).toBe("#3\n#7 A change");
	});
});

describe("pullPrompts", () => {
	const tasks = (over: Partial<PullSnapshot>) => pullPrompts(read(over)).map((p) => p.task);

	it("offers fixes only for an open PR that needs them", () => {
		expect(tasks({})).toEqual(["explain"]);
		expect(tasks({ checks: "failing" })).toEqual(["checks", "explain"]);
		expect(tasks({ mergeable: "conflicting" })).toEqual(["conflicts", "explain"]);
		expect(tasks({ state: "closed", checks: "failing", mergeable: "conflicting" })).toEqual([
			"explain",
		]);
		expect(pullPrompts(pullLink(7)).map((p) => p.task)).toEqual(["explain"]);
	});

	it("keeps messages sent to agents independent of the interface language", () => {
		setLanguageChoice("en");
		const link = read({ checks: "failing", mergeable: "conflicting" });
		const expected = pullPrompts(link).map((prompt) => prompt.text);
		i18n.addResourceBundle("nl", "translation", {
			"pull request #{{number}} in {{repo}}": "vertaalde PR {{number}} in {{repo}}",
		});
		setLanguageChoice("nl");
		try {
			expect(pullPrompts(link).map((prompt) => prompt.text)).toEqual(expected);
		} finally {
			setLanguageChoice("en");
		}
	});

	it("names the PR's number and repository in each message", () => {
		for (const prompt of pullPrompts(read({ checks: "failing", mergeable: "conflicting" }))) {
			expect(prompt.text).toContain(`pull request #7 in ${REPO}`);
		}
	});
});

describe("pullUrl", () => {
	it("is the PR's page on GitHub", () => {
		expect(pullUrl({ repo: REPO, number: 7 })).toBe("https://github.com/owner/repo/pull/7");
	});
});
