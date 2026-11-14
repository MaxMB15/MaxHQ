// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ProjectSettings } from "../bindings";
import {
	allowedWorkflowEnvText,
	draftOf,
	parseMemory,
	planProjectSettings,
	sameDraft,
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
