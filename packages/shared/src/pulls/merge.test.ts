// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { detail } from "../test/pulls";

import type { GithubSignIn } from "./devices";
import { allowedMergeMethods, MERGE_CONFLICT_REASON, mergeUnavailableReason } from "./merge";

const READY: GithubSignIn = { state: "ready", device: "dev-mac", name: "Mac", login: "you" };

describe("allowedMergeMethods", () => {
	it("gives the repository's methods in menu order", () => {
		const pr = detail(1, {}, { merge_methods: ["rebase", "merge", "squash"] });
		expect(allowedMergeMethods(pr)).toEqual(["squash", "merge", "rebase"]);
		expect(allowedMergeMethods(detail(1, {}, { merge_methods: ["merge"] }))).toEqual(["merge"]);
	});
});

describe("mergeUnavailableReason", () => {
	it("is null for an open PR you can merge", () => {
		expect(mergeUnavailableReason(detail(1), READY)).toBeNull();
	});

	it("says why a PR can't merge", () => {
		const reason = (pr: ReturnType<typeof detail>) => mergeUnavailableReason(pr, READY);
		expect(reason(detail(1, { state: "merged" }))).toBe("This PR has been merged.");
		expect(reason(detail(1, { state: "closed" }))).toBe("Reopen this PR before merging.");
		expect(reason(detail(1, { draft: true }))).toBe(
			"Mark this draft ready for review before merging."
		);
		expect(reason(detail(1, { mergeable: "conflicting" }))).toBe(MERGE_CONFLICT_REASON);
		expect(reason(detail(1, {}, { merge_methods: [] }))).toBe(
			"This repository has no available merge methods."
		);
	});

	it("needs your sign-in on a device that's online", () => {
		expect(mergeUnavailableReason(detail(1), { state: "none" })).toBe(
			"Sign in to GitHub in Settings to merge here."
		);
		expect(mergeUnavailableReason(detail(1), { ...READY, state: "offline" })).toBe(
			"Merging needs Mac, which is offline."
		);
	});

	it("says a PR is merged before asking for a sign-in", () => {
		expect(mergeUnavailableReason(detail(1, { state: "merged" }), { state: "none" })).toBe(
			"This PR has been merged."
		);
	});
});
