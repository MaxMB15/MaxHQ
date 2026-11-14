// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { account, agent } from "../test/build";

import {
	canEnd,
	currentModel,
	modelLabel,
	modelName,
	moveModels,
	permissionChoices,
	permissionLabel,
	permissionNote,
	picksModel,
	runningMode,
} from "./controls";

const models = [
	{ id: "opus", label: "Opus 4.8", default: true },
	{ id: "sonnet", label: "Sonnet 4.6", default: false },
];

describe("permission", () => {
	it("lists every mode for Claude and Cursor, and Plan as not in Codex", () => {
		expect(permissionChoices("claude").map((c) => [c.label, c.offered])).toEqual([
			["Ask first", true],
			["Accept edits", true],
			["Auto", true],
			["Plan only", true],
			["Full access", true],
		]);
		expect(permissionChoices("cursor").every((c) => c.offered)).toBe(true);
		expect(permissionChoices("codex").find((c) => c.mode === "plan")?.offered).toBe(false);
		expect(permissionChoices("claude").find((c) => c.mode === "auto")?.detail).toBe(
			"Runs the repo's own test, build and lint scripts without asking. Use it on repos you trust"
		);
	});

	it("hides what a local ACP agent can't do", () => {
		expect(permissionChoices("acp").map((c) => c.mode)).toEqual([
			"ask",
			"accept_edits",
			"full_access",
		]);
		expect(permissionChoices("acp", { plan: true }).map((c) => c.mode)).toContain("plan");
	});

	it("runs in the mode the CLI reports, and says so when it differs", () => {
		const asked = { requested: "full_access" as const, effective: "ask" as const };
		expect(runningMode(asked)).toBe("ask");
		expect(runningMode({ requested: "auto", effective: null })).toBe("auto");
		expect(permissionNote(asked)).toBe("Runs as Ask first. You asked for Full access.");
		expect(permissionNote({ requested: "auto", effective: null })).toBeNull();
		expect(permissionNote({ requested: "auto", effective: "auto" })).toBeNull();
		expect(permissionLabel("accept_edits")).toBe("Accept edits");
	});
});

describe("model", () => {
	it("labels a model the account doesn't list by its name", () => {
		expect(modelLabel("haiku", models)).toBe("haiku");
		expect(modelLabel(null, models)).toBe("Opus 4.8");
		expect(modelLabel(null, [])).toBe("Default model");
	});

	it("marks the agent's model, else the default", () => {
		expect(currentModel("sonnet", models)).toBe("sonnet");
		expect(currentModel(null, models)).toBe("opus");
		expect(currentModel(null, [])).toBe("");
	});

	it("names a Claude model by its name rather than its id", () => {
		expect(modelName("claude-opus-5-5")).toBe("Opus 5.5");
		expect(modelName("claude-sonnet-4-5-20250929")).toBe("Sonnet 4.5");
		expect(modelName("claude-opus-4-20250514")).toBe("Opus 4");
		expect(modelName("claude-3-5-haiku-20241022")).toBe("Haiku 3.5");
		expect(modelName("claude-opus-5-5[1m]")).toBe("Opus 5.5 (1M)");
		expect(modelName("gpt-5.5-codex")).toBeNull();
		expect(modelName("claude-sonnet-latest")).toBeNull();
		expect(modelLabel("claude-haiku-4-5", models)).toBe("Haiku 4.5");
		expect(modelLabel("gpt-5.5-codex", models)).toBe("gpt-5.5-codex");
	});

	it("finds the alias a Claude model id runs, by its name", () => {
		const claude = [
			{ id: "opus", label: "Opus 5.5", default: true },
			{ id: "sonnet", label: "Sonnet 5.5", default: false },
		];
		expect(modelLabel("claude-sonnet-5-5", claude)).toBe("Sonnet 5.5");
		expect(currentModel("claude-sonnet-5-5", claude)).toBe("sonnet");
		expect(currentModel("claude-haiku-4-5", claude)).toBe("claude-haiku-4-5");
	});

	it("offers the default model on an account that lists none", () => {
		const codex = account({ id: "codex", vendor: "codex" });
		expect(moveModels({ account: codex, models: [] })).toEqual([
			{ id: "", label: "Default model" },
		]);
		expect(moveModels({ account: codex, models })).toBe(models);
	});

	it("lets you pick the model of a lead MaxHQ holds", () => {
		const held = agent({ id: "a", control: "held" });
		const move = { account: account({ id: "codex", vendor: "codex" }), models: [] };
		expect(picksModel(held, models, [])).toBe(true);
		expect(picksModel(held, [], [move])).toBe(true);
		expect(picksModel(held, [], [])).toBe(false);
		expect(picksModel({ ...held, control: "observed" }, models, [])).toBe(false);
		expect(picksModel({ ...held, parent: "lead" }, models, [])).toBe(false);
	});
});

describe("canEnd", () => {
	it("ends a held agent with a live session", () => {
		const held = agent({ id: "a", control: "held", session: "s1" });
		expect(canEnd(held)).toBe(true);
		expect(canEnd({ ...held, session: null })).toBe(false);
		expect(canEnd({ ...held, control: "observed" })).toBe(false);
		expect(canEnd({ ...held, link: { state: "down", reason: "gone", since: 0 } })).toBe(false);
	});
});
