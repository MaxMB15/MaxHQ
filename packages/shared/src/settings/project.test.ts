// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, ProjectSettings } from "../bindings";
import { entities, link } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	allowedWorkflowEnvText,
	commandRulesOn,
	deviceSettleDays,
	draftOf,
	parseMemory,
	planProjectSettings,
	projectCapabilities,
	ruleKey,
	sameDraft,
	shownDraft,
} from "./project";

const GIB = 1024 ** 3;

const settings: ProjectSettings = {
	project: "proj-maxhq",
	device: "dev-devbox",
	resume_on_restart: true,
	ignored_ports: [],
	git_author: null,
	memory_soft: 3 * GIB,
};

describe("allowed workflow variables", () => {
	const at = 1_000_000;
	const kept: ProjectSettings = {
		...settings,
		workflow_env: true,
		workflow_env_allowed: {
			vars: [
				{ name: "EXAMPLE_MODE", digest: "a" },
				{ name: "EXAMPLE_TOKEN", digest: "b" },
			],
			at,
		},
	};

	it("names what you allowed and how to forget it", () => {
		expect(allowedWorkflowEnvText(kept, at + 5 * 60_000)).toBe(
			"You allowed EXAMPLE_MODE, EXAMPLE_TOKEN for every agent here 5 min ago. Turn this off and save to forget it."
		);
	});

	it("says nothing while the device keeps none, or has the variables off", () => {
		expect(allowedWorkflowEnvText(settings, at)).toBeNull();
		expect(allowedWorkflowEnvText({ ...kept, workflow_env: false }, at)).toBeNull();
	});

	it("never sends what maxd keeps itself", () => {
		const plan = planProjectSettings(kept.project, kept.device, draftOf(kept));
		expect(plan.ok && "workflow_env_allowed" in plan.settings).toBe(false);
	});
});

describe("project memory setting", () => {
	it("reads the limit in GiB, empty for the host spec's", () => {
		expect(parseMemory("")).toEqual({ ok: true, bytes: null });
		expect(parseMemory(" 2.5 GiB ")).toEqual({ ok: true, bytes: 2.5 * GIB });
		expect(parseMemory("4")).toEqual({ ok: true, bytes: 4 * GIB });
		expect(parseMemory("0").ok).toBe(false);
		expect(parseMemory("lots")).toEqual({
			ok: false,
			reason: "lots isn't a memory limit. Use GiB above 0 and up to 1,024, or leave it empty for the host's.",
		});
	});

	it("keeps the limit through the form and back", () => {
		const draft = draftOf(settings);
		expect(draft.memory).toBe("3");
		const plan = planProjectSettings(settings.project, settings.device, draft);
		expect(plan).toMatchObject({ ok: true, settings: { memory_soft: 3 * GIB } });
		expect(draftOf({ ...settings, memory_soft: null }).memory).toBe("");
		expect(sameDraft(draft, { ...draft, memory: "4" })).toBe(false);
	});

	it("won't save a limit that doesn't read", () => {
		const plan = planProjectSettings(settings.project, settings.device, {
			...draftOf(settings),
			memory: "-1",
		});
		expect(plan.ok).toBe(false);
	});
});

describe("the form a device's settings show", () => {
	const current = draftOf(settings);
	const start = { ...current, name: "Example Author", email: "author@example.com" };
	const typed = { ...current, ports: "9229" };

	it("keeps what you typed while the device reports the same", () => {
		expect(shownDraft({ draft: typed, from: current }, current, start)).toBe(typed);
	});

	it("starts again once the device reports something new, or nothing was typed", () => {
		const changed = { ...current, resume: false };
		expect(shownDraft({ draft: typed, from: current }, changed, start)).toBe(start);
		expect(shownDraft(null, current, start)).toBe(start);
	});
});

describe("what a device keeps for a project", () => {
	const view = applySnapshot({
		seq: 1,
		entities: [
			...entities({
				links: [
					link({
						id: "devbox",
						device: "dev-devbox",
						capabilities: {
							transfers: true,
							tunnels: true,
							workspaces: true,
							inbox: true,
							pulls: true,
						},
					}),
					link({ id: "old", device: "dev-old" }),
				],
			}),
			{
				entity: "device_settings",
				device: "dev-devbox",
				default_permission: "auto",
				workspace_network: null,
				workspace_pool: null,
				auto_settle_days: 9,
			} as Entity,
			{
				entity: "command_rules",
				project: "proj-maxhq",
				device: "dev-devbox",
				rules: [
					{ command: "git show", exact: false },
					{ command: "git show", exact: true },
				],
			},
		],
	});

	it("says which settings each maxd is new enough for", () => {
		expect(projectCapabilities(view, "dev-devbox")).toEqual({
			settle: true,
			pulls: true,
			workflows: false,
			memory: false,
		});
		expect(projectCapabilities(view, "dev-old")).toEqual({
			settle: false,
			pulls: false,
			workflows: false,
			memory: false,
		});
	});

	it("reads the device's settle days, or the default", () => {
		expect(deviceSettleDays(view, "dev-devbox")).toBe(9);
		expect(deviceSettleDays(view, "dev-old")).toBe(5);
	});

	it("lists the command rules, the same empty list when there are none", () => {
		const rules = commandRulesOn(view, "proj-maxhq", "dev-devbox");
		expect(rules.map(ruleKey)).toEqual(["~git show", "=git show"]);
		expect(commandRulesOn(view, "proj-maxhq", "dev-old")).toEqual([]);
		expect(commandRulesOn(view, "proj-maxhq", "dev-old")).toBe(
			commandRulesOn(view, "proj-other", "dev-devbox")
		);
	});
});
