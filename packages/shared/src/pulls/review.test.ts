// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { PullDetail, PullReview } from "../bindings";
import { detail } from "../test/pulls";

import { actedNote, latestVerdicts, mergeability } from "./review";

const review = (author: string, verdict: PullReview["verdict"], at: number): PullReview => ({
	author,
	verdict,
	commit_sha: "abc",
	at,
});

describe("latestVerdicts", () => {
	it("keeps each reviewer's last verdict, past later comments", () => {
		const shown = latestVerdicts([
			review("sam", "changes_requested", 1),
			review("alex", "commented", 2),
			review("Sam", "approved", 3),
			review("sam", "commented", 4),
			review("alex", "approved", 5),
		]);
		expect(shown.map((r) => [r.author, r.verdict])).toEqual([
			["Sam", "approved"],
			["alex", "approved"],
		]);
	});
});

describe("mergeability", () => {
	const withPull = (pull: Partial<PullDetail["summary"]["pull"]>): PullDetail => {
		const pr = detail(1);
		return { ...pr, summary: { ...pr.summary, pull: { ...pr.summary.pull, ...pull } } };
	};

	it("says whether the PR merges cleanly, and nothing once it's closed", () => {
		expect(mergeability(withPull({ mergeable: "yes", base: "main" }))).toBe(
			"No conflicts with main."
		);
		expect(mergeability(withPull({ mergeable: "unknown" }))).toBe(
			"GitHub is still checking for conflicts."
		);
		expect(mergeability(withPull({ state: "merged" }))).toBeNull();
	});
});

describe("actedNote", () => {
	it("says what a merge or an approval came to, and a refusal as an error", () => {
		expect(actedNote("merge", { state: "done" })).toEqual({ text: "Merged.", error: false });
		expect(actedNote("merge", { state: "queued" })).toEqual({
			text: "Queued to merge.",
			error: false,
		});
		expect(actedNote("approve", { state: "done" })).toEqual({
			text: "Approved.",
			error: false,
		});
		expect(actedNote("approve", { state: "refused", reason: "No." })).toEqual({
			text: "No.",
			error: true,
		});
	});
});
