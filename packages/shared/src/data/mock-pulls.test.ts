// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { NOW } from "../test/build";

import {
	FILES_PAGE,
	MOCK_GITHUB_LOGIN,
	MOCK_PULLS_PROJECT,
	MOCK_REPO,
	mockPulls,
	SIGN_IN_MS,
} from "./mock-pulls";
import type { ActionResult } from "./source";

const sample = (login: string | null = MOCK_GITHUB_LOGIN) => {
	const changed = vi.fn();
	const signedIn = vi.fn();
	const repoOf = (agent: string) =>
		agent === "elsewhere" ? "owner/other" : "Example-User/MaxHQ";
	const mock = mockPulls({ now: () => NOW, changed, repoOf, login: () => login, signedIn });
	return { ...mock, changed, signedIn };
};

/** The outcome of an answer that has one. */
const outcome = (result: ActionResult) => {
	if (result.status !== "ok") throw new Error("no outcome");
	return result.data;
};

const list = async (mock: ReturnType<typeof sample>, state: "open" | "closed") => {
	const answer = outcome(await mock.calls.listPulls(MOCK_PULLS_PROJECT, state));
	return answer.status === "pulls" ? answer.pulls : [];
};

const head = async (mock: ReturnType<typeof sample>, number: number) => {
	const answer = outcome(await mock.calls.getPull("dev-devbox", MOCK_REPO, number));
	if (answer.status !== "pull") throw new Error("no PR");
	return answer.pull;
};

afterEach(() => {
	vi.useRealTimers();
});

describe("mockPulls", () => {
	it("lists each kind of open PR, and the merged and closed ones apart", async () => {
		const mock = sample();
		const open = (await list(mock, "open")).map((row) => row.pull);
		expect(open.some((p) => p.checks === "passing" && p.review === "approved")).toBe(true);
		expect(open.some((p) => p.checks === "failing")).toBe(true);
		expect(open.some((p) => p.checks === "pending")).toBe(true);
		expect(open.some((p) => p.draft)).toBe(true);
		expect(open.some((p) => p.mergeable === "conflicting")).toBe(true);
		const closed = (await list(mock, "closed")).map((row) => row.pull.state).sort();
		expect(closed).toEqual(["closed", "merged"]);
		expect(outcome(await mock.calls.listPulls("proj-comet", "open"))).toEqual({
			status: "pulls",
			pulls: [],
		});
	});

	it("lists the repo's branches: main and every PR's head", async () => {
		const mock = sample();
		const answer = outcome(await mock.calls.listBranches(MOCK_PULLS_PROJECT));
		if (answer.status !== "branches") throw new Error("no branches");
		expect(answer.default_branch).toBe("main");
		expect(answer.branches).toContain("main");
		expect(answer.branches).toContain("usage-week");
		expect(outcome(await mock.calls.listBranches("proj-comet"))).toEqual({
			status: "branches",
			branches: ["main"],
			default_branch: "main",
		});
	});

	it("names the agents each PR is linked to, leaving out dismissed links", async () => {
		const mock = sample();
		const rows = await list(mock, "open");
		expect(rows.find((row) => row.key.number === 142)?.agents).toEqual(["agent-usage-popover"]);
		expect(rows.find((row) => row.key.number === 144)?.agents).toEqual([]);
		expect(mock.linksOf("agent-bindings")?.[0]?.source).toBe("dismissed");
		expect(mock.linksOf("agent-hdr")).toBeUndefined();
	});

	it("pages a PR's files", async () => {
		const mock = sample();
		const first = outcome(await mock.calls.pullFiles("dev-devbox", MOCK_REPO, 145, 1));
		const second = outcome(await mock.calls.pullFiles("dev-devbox", MOCK_REPO, 145, 2));
		expect(first).toMatchObject({ status: "pull_files", more: true });
		expect(first.status === "pull_files" && first.files.length).toBe(FILES_PAGE);
		expect(second).toMatchObject({ status: "pull_files", more: false });
	});

	it("reads a conversation and checks of each state", async () => {
		const mock = sample();
		const comments = outcome(await mock.calls.pullComments("dev-devbox", MOCK_REPO, 141));
		expect(comments.status === "pull_comments" && comments.comments.length).toBeGreaterThan(1);
		const states = new Set<string>();
		for (const number of [141, 142, 145]) {
			const checks = outcome(await mock.calls.pullChecks("dev-devbox", MOCK_REPO, number));
			if (checks.status === "pull_checks") for (const c of checks.checks) states.add(c.state);
		}
		expect([...states].sort()).toEqual(["failing", "passing", "pending", "skipped"]);
	});

	it("refuses a PR it doesn't have", async () => {
		const mock = sample();
		expect(outcome(await mock.calls.getPull("dev-devbox", "owner/other", 141)).status).toBe(
			"refused"
		);
	});

	it("merges at the head shown, and moves the PR to the Closed tab", async () => {
		const mock = sample();
		const pr = await head(mock, 141);
		const moved = await mock.calls.pullAction(
			MOCK_REPO,
			141,
			{ action: "merge", method: "squash" },
			"old"
		);
		expect(outcome(moved)).toMatchObject({ status: "refused" });
		const merged = await mock.calls.pullAction(
			MOCK_REPO,
			141,
			{ action: "merge", method: "squash" },
			pr.summary.pull.head_sha
		);
		expect(outcome(merged)).toEqual({ status: "pull_merged", queued: false });
		expect((await head(mock, 141)).summary.pull.state).toBe("merged");
		expect((await list(mock, "closed")).some((row) => row.key.number === 141)).toBe(true);
		expect(mock.linksOf("agent-release")?.[0]?.snapshot?.state).toBe("merged");
		expect(mock.changed).toHaveBeenCalled();
	});

	it("refuses to merge a draft or a conflicting PR", async () => {
		const mock = sample();
		for (const number of [143, 144]) {
			const sha = (await head(mock, number)).summary.pull.head_sha;
			const answer = await mock.calls.pullAction(
				MOCK_REPO,
				number,
				{ action: "merge", method: "merge" },
				sha
			);
			expect(outcome(answer).status).toBe("refused");
		}
	});

	it("approves someone else's PR and refuses your own", async () => {
		const mock = sample();
		const theirs = await head(mock, 144);
		expect(
			outcome(
				await mock.calls.pullAction(
					MOCK_REPO,
					144,
					{ action: "approve" },
					theirs.summary.pull.head_sha
				)
			)
		).toEqual({ status: "done" });
		const reviews = (await head(mock, 144)).reviews;
		expect(reviews[reviews.length - 1]).toMatchObject({
			author: MOCK_GITHUB_LOGIN,
			verdict: "approved",
		});
		const yours = await head(mock, 142);
		expect(
			outcome(
				await mock.calls.pullAction(
					MOCK_REPO,
					142,
					{ action: "approve" },
					yours.summary.pull.head_sha
				)
			)
		).toEqual({ status: "refused", reason: "GitHub doesn't let you approve your own PR." });
	});

	it("needs a GitHub sign-in to merge or approve", async () => {
		const mock = sample(null);
		const answer = await mock.calls.pullAction(MOCK_REPO, 141, { action: "approve" }, "x");
		expect(answer.status).toBe("error");
	});

	it("links and dismisses PRs on an agent", async () => {
		const mock = sample();
		expect(outcome(await mock.calls.linkPull("agent-hdr", 144))).toEqual({ status: "done" });
		expect(mock.linksOf("agent-hdr")?.map((l) => [l.key.number, l.source])).toEqual([
			[144, "you"],
		]);
		await mock.calls.unlinkPull("agent-hdr", "Example-User/MaxHQ", 144);
		expect(mock.linksOf("agent-hdr")?.map((l) => l.source)).toEqual(["dismissed"]);
		expect(outcome(await mock.calls.linkPull("elsewhere", 144)).status).toBe("refused");
	});

	it("gives a code, then signs you in a few seconds later", async () => {
		vi.useFakeTimers();
		const mock = sample();
		const answer = outcome(await mock.calls.beginGithubSignIn("dev-mac", null));
		expect(answer).toMatchObject({ status: "device_code", user_code: "WDJB-MJHT" });
		expect(mock.signedIn).not.toHaveBeenCalled();
		vi.advanceTimersByTime(SIGN_IN_MS);
		expect(mock.signedIn).toHaveBeenCalledExactlyOnceWith("dev-mac", MOCK_GITHUB_LOGIN);
	});

	it("signs nobody in once the code is cancelled", async () => {
		vi.useFakeTimers();
		const mock = sample();
		const answer = outcome(await mock.calls.beginGithubSignIn("dev-mac", null));
		if (answer.status !== "device_code") throw new Error("no code");
		await mock.calls.cancelGithubSignIn("dev-mac", answer.sign_in);
		vi.advanceTimersByTime(SIGN_IN_MS);
		expect(mock.signedIn).not.toHaveBeenCalled();
	});
});
