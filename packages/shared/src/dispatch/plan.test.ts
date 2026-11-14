// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { PermissionMode, Plan, PlanItem } from "../bindings";
import { account, agent, device, entities, link, project, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	dispatcherAccount,
	dispatcherAccounts,
	itemLines,
	itemMode,
	itemPhrase,
	itemPreview,
	outcomeLine,
	planSummary,
} from "./plan";

const view = applySnapshot({
	seq: 1,
	entities: entities({
		projects: [project("p-comet", "Comet")],
		devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-box" })],
		links: [link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } })],
		workspaces: [
			workspace({
				id: "w-comet",
				project: "p-comet",
				location: { type: "folder", path: "/work/Comet" },
			}),
		],
		accounts: [
			account({ id: "codex", vendor: "codex", label: "Codex" }),
			account({ id: "claude-b", label: "Work" }),
			account({ id: "claude-a", label: "Personal" }),
			account({ id: "cursor", vendor: "cursor", label: "Cursor" }),
			account({ id: "box-claude", device: "dev-box", label: "Box" }),
		],
		agents: [agent({ id: "reviewer", title: "Reviewer", workspace: "w-comet" })],
	}),
});

const plan: Plan = {
	id: 4,
	items: [
		{
			reason: "Reviews octocat's PRs",
			step: {
				type: "start_agent",
				device: "dev-mac",
				spec: {
					vendor: "cursor",
					account: "cursor",
					place: { type: "folder", path: "/work/Comet" },
					model: null,
					permission: null,
					title: "PR reviews",
					first: null,
					branch: null,
				},
			},
		},
		{
			reason: "Sends it each batch of octocat's PR changes",
			step: {
				type: "schedule_task",
				device: "dev-mac",
				name: "Review octocat's PRs",
				prompt: "Review the PR changes in this batch.",
				trigger: {
					type: "github_events",
					repo: "owner/repo",
					events: ["pull_request"],
					rules: {
						filters: [
							{
								path: "body.pull_request.user.login",
								test: { op: "equals", value: "octocat" },
							},
						],
						replay_key: null,
						coalesce_key: "octocat",
						coalesce_secs: 900,
						coalesce_restart: true,
						coalesce_max_secs: 3600,
						max_age_minutes: null,
						daily_runs: null,
					},
				},
				target: { type: "item", item: 0 },
			},
		},
	],
};

describe("the dispatcher's account", () => {
	it("offers this device's Claude accounts, then its Codex ones", () => {
		expect(dispatcherAccounts(view, "dev-mac").map((a) => a.id)).toEqual([
			"claude-a",
			"claude-b",
			"codex",
		]);
	});

	it("keeps the one you picked, else takes the first", () => {
		expect(dispatcherAccount(view, "dev-mac", "codex")?.id).toBe("codex");
		expect(dispatcherAccount(view, "dev-mac", "box-claude")?.id).toBe("claude-a");
		expect(dispatcherAccount(view, "dev-mac", null)?.id).toBe("claude-a");
		expect(dispatcherAccount(view, null, null)).toBeUndefined();
	});
});

describe("a plan in words", () => {
	it("sums the plan up in one line", () => {
		expect(planSummary(view, plan)).toBe(
			"2 items: a Cursor agent in Comet on Mac and a task on owner/repo PR events"
		);
	});

	it("names a message and a timed task", () => {
		const other: Plan = {
			id: 5,
			items: [
				{
					reason: "Asks for a rebase",
					step: {
						type: "send_message",
						device: "dev-mac",
						agent: "reviewer",
						text: "Rebase",
					},
				},
				{
					reason: "Checks in",
					step: {
						type: "schedule_task",
						device: "dev-mac",
						name: "Check in",
						prompt: "How's it going?",
						trigger: { type: "every", minutes: 15 },
						target: {
							type: "task",
							target: { type: "agent", device: "dev-mac", agent: "reviewer" },
						},
					},
				},
			],
		};
		expect(planSummary(view, other)).toBe(
			"2 items: a message to Reviewer and a task that runs every 15 min"
		);
	});

	it("opens a started agent to all it is", () => {
		expect(itemLines(view, plan, 0)).toEqual([
			{ label: "Account", value: "Cursor" },
			{ label: "Device", value: "Mac" },
			{ label: "Where", value: "/work/Comet" },
			{ label: "Model", value: "Default" },
			{ label: "Mode", value: "Auto" },
			{ label: "Title", value: "PR reviews" },
			{ label: "First message", value: "None, it starts idle" },
		]);
	});

	it("opens a task to its filters and wait", () => {
		expect(itemLines(view, plan, 1)).toEqual([
			{ label: "Name", value: "Review octocat's PRs" },
			{ label: "Device", value: "Mac" },
			{
				label: "Runs",
				value: "On pull requests in owner/repo. Waits 15 min after the last event, 1 h at most",
			},
			{ label: "Only when", value: "body.pull_request.user.login equals octocat" },
			{ label: "Sends to", value: "The new Cursor agent in Comet on Mac" },
			{ label: "Prompt", value: "Review the PR changes in this batch." },
		]);
	});

	it("says what a new agent may do and what it's told before you open the item", () => {
		const start = (
			device: string,
			permission: PermissionMode | null,
			text: string | null
		): PlanItem => ({
			reason: "Builds it",
			step: {
				type: "start_agent",
				device,
				spec: {
					vendor: "cursor",
					account: "cursor",
					place: { type: "folder", path: "/work/Comet" },
					model: null,
					permission,
					title: null,
					first: text === null ? null : { text, attachments: [], refs: [] },
					branch: null,
				},
			},
		});
		// What the plan asks for wins, and a null takes the device's default.
		expect(itemMode(view, start("dev-mac", null, null))).toBe("auto");
		expect(itemMode(view, start("dev-mac", "full_access", null))).toBe("full_access");
		expect(itemMode(view, start("dev-box", "ask", null))).toBe("ask");
		expect(itemMode(view, plan.items[1]!)).toBeNull();

		// Full access is named in the words every view shows.
		expect(itemPhrase(view, start("dev-mac", "full_access", null))).toBe(
			"a Cursor agent in Comet on Mac, with Full access"
		);
		expect(itemPhrase(view, start("dev-mac", "auto", null))).toBe(
			"a Cursor agent in Comet on Mac"
		);

		// Blank lines can't push the words below what shows.
		expect(itemPreview(start("dev-mac", null, null))).toBeNull();
		expect(itemPreview(start("dev-mac", null, "Hello\nthere"))).toBe("Hello\nthere");
		expect(itemPreview(start("dev-mac", null, "\n\n\nRead\n\nthe\n\nkeys\nand send"))).toBe(
			"Read\nthe\nkeys..."
		);
		expect(itemPreview(start("dev-mac", null, "x".repeat(500)))).toBe(`${"x".repeat(400)}...`);
		expect(itemPreview(plan.items[1]!)).toBe("Review the PR changes in this batch.");

		// A pill the dispatcher passed on shows as its words.
		const pilled = start("dev-mac", null, "Ship it like \uFFFC");
		if (pilled.step.type !== "start_agent" || !pilled.step.spec.first) throw new Error("first");
		pilled.step.spec.first.refs = [
			{
				type: "agent",
				agent: "a2",
				title: "Release notes",
				vendor: "claude",
				device: "dev-mac",
				excerpt: null,
			},
		];
		expect(itemPreview(pilled)).toBe("Ship it like Release notes");
	});

	it("says when a task keeps to the repo's people", () => {
		const step = plan.items[1]?.step;
		if (step?.type !== "schedule_task" || step.trigger.type !== "github_events")
			throw new Error("the plan's task");
		const rules = { ...step.trigger.rules, authors: ["OWNER", "MEMBER", "COLLABORATOR"] };
		const kept: Plan = {
			...plan,
			items: [
				plan.items[0]!,
				{ ...plan.items[1]!, step: { ...step, trigger: { ...step.trigger, rules } } },
			],
		};
		expect(itemLines(view, kept, 1)).toContainEqual({
			label: "Only for",
			value: "The repo's owners, members and collaborators",
		});
		expect(itemLines(view, plan, 1).map((line) => line.label)).not.toContain("Only for");
	});

	it("says how each item went", () => {
		expect(outcomeLine({ type: "started", agent: "a" })).toBe("Started");
		expect(outcomeLine({ type: "task_saved", task: "t" })).toBe("Task saved");
		expect(outcomeLine({ type: "unknown" })).toBe("Done; update the app for details");
		expect(outcomeLine({ type: "failed", reason: "Item 0 didn't start an agent." })).toBe(
			"Item 0 didn't start an agent."
		);
	});
});
