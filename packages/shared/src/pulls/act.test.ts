// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { Entity, PullReview } from "../bindings";
import type { ActionResult, DataSource } from "../data/source";
import { agent, device, entities, link, NOW, workspace } from "../test/build";
import { detail, githubUser, pullLink, REPO } from "../test/pulls";
import { applySnapshot } from "../view/replica";

import { approvedByYou, approvePull, approveUnavailableReason, mergePull } from "./act";
import type { GithubSignIn } from "./devices";

const OK = (data: ActionResult & { status: "ok" }): ActionResult => data;
const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const viewWith = (extra: Entity[]) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [
					device({ id: "dev-mac", name: "Mac" }),
					device({ id: "dev-box", name: "box" }),
				],
				links: [link({ id: "mac", name: "Mac" }), link({ id: "box", name: "box" })],
				workspaces: [
					workspace({ id: "w-mac", device: "dev-mac" }),
					workspace({ id: "w-box", device: "dev-box" }),
				],
				agents: [
					agent({ id: "a1", workspace: "w-mac", pulls: [pullLink(7)] }),
					agent({ id: "a2", workspace: "w-box", pulls: [pullLink(7, "branch")] }),
					agent({ id: "a3", workspace: "w-box", pulls: [pullLink(7, "dismissed")] }),
				],
			}),
			...extra,
		],
	});

const SIGNED_IN = viewWith([githubUser("dev-mac")]);
const AT = { key: { repo: REPO, number: 7 }, headSha: "abc123" };

const source = (answer: ActionResult) => ({
	pullAction: vi.fn<DataSource["pullAction"]>(async () => answer),
	pullsDue: vi.fn<DataSource["pullsDue"]>(async () => DONE),
});

const review = (author: string, verdict: PullReview["verdict"], sha = "abc123"): PullReview => ({
	author,
	verdict,
	commit_sha: sha,
	at: NOW,
});

describe("mergePull", () => {
	it("merges at the head shown, then has each linked agent's device read again", async () => {
		const s = source(OK({ status: "ok", data: { status: "pull_merged", queued: false } }));
		expect(await mergePull(s, SIGNED_IN, NOW, AT, "squash")).toEqual({ state: "done" });
		expect(s.pullAction).toHaveBeenCalledExactlyOnceWith(
			REPO,
			7,
			{ action: "merge", method: "squash" },
			"abc123"
		);
		expect(s.pullsDue.mock.calls.map(([device]) => device).sort()).toEqual([
			"dev-box",
			"dev-mac",
		]);
		expect(s.pullsDue).toHaveBeenCalledWith("dev-mac", REPO);
	});

	it("says queued, and tells no device, when the PR is still open", async () => {
		const s = source(OK({ status: "ok", data: { status: "pull_merged", queued: true } }));
		expect(await mergePull(s, SIGNED_IN, NOW, AT, "merge")).toEqual({ state: "queued" });
		expect(s.pullsDue).not.toHaveBeenCalled();
	});

	it("gives the refusal's reason", async () => {
		const s = source(
			OK({ status: "ok", data: { status: "refused", reason: "The PR changed." } })
		);
		expect(await mergePull(s, SIGNED_IN, NOW, AT, "merge")).toEqual({
			state: "refused",
			reason: "The PR changed.",
		});
		expect(s.pullsDue).not.toHaveBeenCalled();
	});

	it("gives an error in words", async () => {
		const s = source({ status: "error", error: { kind: "failed", message: "No route." } });
		expect(await mergePull(s, SIGNED_IN, NOW, AT, "merge")).toEqual({
			state: "refused",
			reason: "No route.",
		});
	});

	it("sends nothing without a sign-in", async () => {
		const s = source(DONE);
		expect(await mergePull(s, viewWith([]), NOW, AT, "merge")).toEqual({
			state: "refused",
			reason: "Sign in to GitHub in Settings to merge here.",
		});
		expect(s.pullAction).not.toHaveBeenCalled();
	});
});

describe("approvePull", () => {
	it("approves the head shown", async () => {
		const s = source(DONE);
		expect(await approvePull(s, SIGNED_IN, NOW, AT)).toEqual({ state: "done" });
		expect(s.pullAction).toHaveBeenCalledExactlyOnceWith(
			REPO,
			7,
			{ action: "approve" },
			"abc123"
		);
	});

	it("gives the refusal's reason", async () => {
		const s = source(OK({ status: "ok", data: { status: "refused", reason: "Not open." } }));
		expect(await approvePull(s, SIGNED_IN, NOW, AT)).toEqual({
			state: "refused",
			reason: "Not open.",
		});
	});

	it("sends nothing when the holder is offline", async () => {
		const offline = viewWith([githubUser("dev-gone")]);
		const s = source(DONE);
		const acted = await approvePull(s, offline, NOW, AT);
		expect(acted).toEqual({
			state: "refused",
			reason: "Approving needs Unknown device, which is offline.",
		});
		expect(s.pullAction).not.toHaveBeenCalled();
	});
});

describe("approvedByYou", () => {
	it("is true when your latest verdict approves the head", () => {
		const pr = detail(
			7,
			{},
			{ reviews: [review("You", "approved"), review("you", "commented")] }
		);
		expect(approvedByYou(pr, "you")).toBe(true);
	});

	it("is false for an approval of an older head, or one you took back", () => {
		expect(
			approvedByYou(detail(7, {}, { reviews: [review("you", "approved", "old")] }), "you")
		).toBe(false);
		const changed = [review("you", "approved"), review("you", "changes_requested")];
		expect(approvedByYou(detail(7, {}, { reviews: changed }), "you")).toBe(false);
		expect(approvedByYou(detail(7, {}, { reviews: [review("them", "approved")] }), "you")).toBe(
			false
		);
	});
});

describe("approveUnavailableReason", () => {
	const ready: GithubSignIn = { state: "ready", device: "dev-mac", name: "Mac", login: "you" };

	it("is null when you can approve", () => {
		expect(approveUnavailableReason(detail(7), ready)).toBeNull();
	});

	it("says why you can't", () => {
		expect(approveUnavailableReason(detail(7, { state: "merged" }), ready)).toBe(
			"This PR isn't open anymore."
		);
		expect(approveUnavailableReason(detail(7), { state: "none" })).toBe(
			"Sign in to GitHub in Settings to approve here."
		);
		expect(approveUnavailableReason(detail(7, { author: "You" }), ready)).toBe(
			"GitHub doesn't let you approve your own PR."
		);
		const approved = detail(7, {}, { reviews: [review("you", "approved")] });
		expect(approveUnavailableReason(approved, ready)).toBe("You already approved this commit.");
	});
});
