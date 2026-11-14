// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { type Account, type Agent, EFFORT_SINCE, type ModelChoice } from "../bindings";
import { account, agent, device, entities, link, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	canSetEffort,
	deviceEffortChoices,
	deviceEffortLevel,
	effortChoices,
	effortLabel,
	effortLevel,
	effortShown,
} from "./effort";

const CLAUDE_LEVELS = ["low", "medium", "high", "xhigh", "max"];

const OPUS: ModelChoice = { id: "opus", label: "Opus", default: true, efforts: CLAUDE_LEVELS };

const MODELS: ModelChoice[] = [OPUS, { id: "haiku", label: "Haiku", default: false }];

const CODEX_MODELS: ModelChoice[] = [
	{
		id: "gpt-sol",
		label: "GPT Sol",
		default: true,
		efforts: ["low", "medium", "high"],
		default_effort: "medium",
	},
];

const view = (protocol: number | null, agents: Agent[] = [], logins: Account[] = []) =>
	applySnapshot({
		seq: 1,
		entities: entities({
			devices: [device({ id: "mac", name: "Mac", os: "macos" })],
			links: [link({ id: "local", device: "mac", protocol })],
			workspaces: [workspace({ id: "w1", device: "mac" })],
			agents,
			accounts: logins,
		}),
	});

const held = (over: Partial<Agent> = {}): Agent =>
	agent({ id: "a1", control: "held", session: "s1", ...over });

describe("effort levels", () => {
	it("lists the model's levels in order, marking the one it runs at when told none", () => {
		const choices = effortChoices(null, "gpt-sol", CODEX_MODELS);
		expect(choices.map((c) => c.value)).toEqual(["low", "medium", "high"]);
		expect(choices.filter((c) => c.isDefault).map((c) => c.value)).toEqual(["medium"]);
		expect(choices.map((c) => c.label)).not.toContain("Vendor default");
	});

	it("falls back to the default model's levels and lists none for a model without them", () => {
		expect(effortChoices(null, null, MODELS).map((c) => c.value)).toEqual(CLAUDE_LEVELS);
		expect(effortChoices(null, "claude-opus-5-5", MODELS)).toHaveLength(5);
		expect(effortChoices(null, "haiku", MODELS)).toEqual([]);
		expect(effortChoices(null, null, [])).toEqual([]);
	});

	it("keeps a level it doesn't know under its own name", () => {
		const choices = effortChoices("ultra", "opus", MODELS);
		expect(choices.at(-1)).toEqual({ value: "ultra", label: "ultra", isDefault: false });
		expect(effortShown("ultra", "opus", MODELS)).toBe("ultra");
		expect(effortLabel("xhigh")).toBe("Extra high");
		expect(effortLabel("turbo")).toBe("turbo");
	});

	it("names the agent's level, else the one its model runs at when told none", () => {
		expect(effortShown("high", "opus", MODELS)).toBe("High");
		expect(effortShown(null, "gpt-sol", CODEX_MODELS)).toBe("Medium");
		const claude: ModelChoice[] = [{ ...OPUS, default_effort: "xhigh" }];
		expect(effortShown(null, "opus", claude)).toBe("Extra high");
		expect(effortChoices(null, "opus", claude).find((c) => c.isDefault)?.value).toBe("xhigh");
	});

	it("resolves a Claude model that names no default to a real level", () => {
		// A maxd from before Claude models named theirs: the CLIs run at high.
		expect(effortLevel(null, "opus", MODELS)).toBe("high");
		expect(effortShown(null, "opus", MODELS)).toBe("High");
		expect(effortChoices(null, "opus", MODELS).find((c) => c.isDefault)?.value).toBe("high");
		// A model without high runs at its middle level.
		const odd: ModelChoice[] = [
			{ id: "x", label: "X", default: true, efforts: ["slow", "steady", "fast"] },
		];
		expect(effortLevel(null, "x", odd)).toBe("steady");
		expect(effortLevel(null, "haiku", MODELS)).toBeNull();
	});
});

describe("canSetEffort", () => {
	it("needs a held lead Claude Code or Codex agent on a maxd that sets levels", () => {
		expect(canSetEffort(view(EFFORT_SINCE), held(), "mac", MODELS)).toBe(true);
		expect(canSetEffort(view(EFFORT_SINCE - 1), held(), "mac", MODELS)).toBe(false);
		expect(canSetEffort(view(null), held(), "mac", MODELS)).toBe(false);
		expect(canSetEffort(view(EFFORT_SINCE), held({ control: "observed" }), "mac", MODELS)).toBe(
			false
		);
		expect(canSetEffort(view(EFFORT_SINCE), held({ parent: "lead" }), "mac", MODELS)).toBe(
			false
		);
		expect(canSetEffort(view(EFFORT_SINCE), held({ vendor: "cursor" }), "mac", MODELS)).toBe(
			false
		);
		expect(canSetEffort(view(EFFORT_SINCE), held({ model: "haiku" }), "mac", MODELS)).toBe(
			false
		);
		expect(canSetEffort(view(EFFORT_SINCE), held(), "box", MODELS)).toBe(false);
	});
});

describe("deviceEffortChoices", () => {
	const logins = [
		account({ id: "claude", device: "mac", models: MODELS }),
		account({ id: "codex", device: "mac", vendor: "codex", models: CODEX_MODELS }),
		account({
			id: "elsewhere",
			device: "box",
			models: [{ id: "x", label: "X", default: true, efforts: ["turbo"] }],
		}),
	];

	it("joins the levels of the device's logins for the vendor", () => {
		const v = view(EFFORT_SINCE, [], logins);
		expect(deviceEffortChoices(v, "mac", "claude", null).map((c) => c.value)).toEqual(
			CLAUDE_LEVELS
		);
		const codex = deviceEffortChoices(v, "mac", "codex", "ultra");
		expect(codex.map((c) => c.value)).toEqual(["low", "medium", "high", "ultra"]);
		expect(codex.filter((c) => c.isDefault).map((c) => c.value)).toEqual(["medium"]);
		expect(deviceEffortChoices(v, "mac", "cursor", null)).toEqual([]);
	});

	it("shows a real level when the device keeps none of its own", () => {
		const v = view(EFFORT_SINCE, [], logins);
		expect(deviceEffortLevel(v, "mac", "codex", null)).toBe("medium");
		expect(deviceEffortLevel(v, "mac", "codex", "high")).toBe("high");
		expect(deviceEffortLevel(v, "mac", "claude", null)).toBe("high");
		expect(deviceEffortLevel(v, "mac", "cursor", null)).toBeNull();
	});
});
