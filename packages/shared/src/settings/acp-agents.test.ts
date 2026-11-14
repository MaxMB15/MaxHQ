// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AcpAgent, Entity } from "../bindings";
import { account, agent, device, entities, link } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import {
	accountOfAgent,
	acpAgentsOn,
	agentOfAccount,
	draftOf,
	keepsHistory,
	needsSignIn,
	newArgument,
	newVariable,
	planAcpSave,
	probeLine,
	runsAcpAgents,
	signInCommand,
	takesImages,
} from "./acp-agents";
import { modeShown, startingMode, vendorHasMode } from "./permission";

const GEMINI: AcpAgent = {
	id: "acp-1",
	device: "dev-mac",
	name: "Gemini",
	program: "gemini",
	args: ["--experimental-acp"],
	env: ["GEMINI_API_KEY"],
	containers: false,
	problem: null,
};

const GEMINI_ACCOUNT = account({
	id: "a-gemini",
	vendor: "acp",
	label: "Gemini",
	config_dir: "",
	acp: { images: true, load: false, plan: true, auth: [] },
});

const viewOf = (acp: boolean, extra: Entity[] = []): ViewState =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [device({ id: "dev-mac", name: "studio", os: "macos" })],
				links: [
					link({
						id: "mac",
						name: "Mac",
						device: "dev-mac",
						route: { type: "local" },
						capabilities: {
							transfers: true,
							tunnels: true,
							workspaces: false,
							acp_agents: acp,
						},
					}),
				],
				accounts: [GEMINI_ACCOUNT],
			}),
			{ entity: "acp_agent", ...GEMINI },
			...extra,
		],
	});

describe("ACP agents on a device", () => {
	it("show only where the device's maxd runs them", () => {
		expect(runsAcpAgents(viewOf(true), "dev-mac")).toBe(true);
		expect(runsAcpAgents(viewOf(false), "dev-mac")).toBe(false);
	});

	it("list by name, and find the account each one runs on", () => {
		const view = viewOf(true, [{ entity: "acp_agent", ...GEMINI, id: "acp-2", name: "Aider" }]);
		expect(acpAgentsOn(view, "dev-mac").map((a) => a.name)).toEqual(["Aider", "Gemini"]);
		expect(accountOfAgent(view, GEMINI)?.id).toBe("a-gemini");
		expect(agentOfAccount(view, GEMINI_ACCOUNT)?.id).toBe("acp-1");
		expect(agentOfAccount(view, account({ id: "a-claude" }))).toBeUndefined();
	});
});

describe("the form", () => {
	const view = viewOf(true);
	const values = (entries: [string, string][]) => new Map(entries);

	it("shows a held value as set, with no field to read", () => {
		const draft = draftOf(GEMINI);
		expect(draft.env.map((r) => [r.name, r.held, r.replacing])).toEqual([
			["GEMINI_API_KEY", true, false],
		]);
		const saved = planAcpSave(view, "dev-mac", draft, GEMINI.id, values([]));
		expect(saved).toMatchObject({ ok: true, env: [] });
		if (saved.ok) expect(saved.agent.env).toEqual(["GEMINI_API_KEY"]);
	});

	it("sends a replaced value, and drops a removed one", () => {
		const draft = draftOf(GEMINI);
		const row = draft.env[0]!;
		const replaced = planAcpSave(
			view,
			"dev-mac",
			{ ...draft, env: [{ ...row, replacing: true }] },
			GEMINI.id,
			values([[row.key, " new-key "]])
		);
		expect(replaced).toMatchObject({
			ok: true,
			env: [{ name: "GEMINI_API_KEY", value: "new-key" }],
		});
		const removed = planAcpSave(view, "dev-mac", { ...draft, env: [] }, GEMINI.id, values([]));
		expect(removed).toMatchObject({ ok: true, agent: { env: [] }, env: [] });
	});

	it("sends new arguments and variables, and skips blank rows", () => {
		const variable = { ...newVariable(), name: "TOKEN" };
		const draft = {
			...draftOf(),
			name: " Goose ",
			program: "goose",
			args: [{ ...newArgument(), value: "acp" }, newArgument()],
			env: [variable, newVariable()],
		};
		const saved = planAcpSave(view, "dev-mac", draft, "acp-9", values([[variable.key, "t"]]));
		expect(saved).toEqual({
			ok: true,
			agent: {
				id: "acp-9",
				device: "dev-mac",
				name: "Goose",
				program: "goose",
				args: ["acp"],
				env: ["TOKEN"],
				containers: false,
			},
			env: [{ name: "TOKEN", value: "t" }],
		});
	});

	it("says what's missing or wrong before anything goes", () => {
		const reason = (over: Partial<ReturnType<typeof draftOf>>, typed = values([])) => {
			const saved = planAcpSave(view, "dev-mac", { ...draftOf(), ...over }, "acp-9", typed);
			return saved.ok ? null : saved.reason;
		};
		expect(reason({ program: "x" })).toBe("Give the agent a name.");
		expect(reason({ name: "X" })).toBe("Give the program to run.");
		expect(reason({ name: "Gemini", program: "x" })).toBe(
			"Mac already has an agent called Gemini."
		);
		const bad = { ...newVariable(), name: "1BAD" };
		expect(reason({ name: "X", program: "x", env: [bad] }, values([[bad.key, "v"]]))).toBe(
			"1BAD isn't a variable name. Use letters, digits and _, and don't start with a digit."
		);
		const a = { ...newVariable(), name: "KEY" };
		const b = { ...newVariable(), name: "KEY" };
		expect(
			reason(
				{ name: "X", program: "x", env: [a, b] },
				values([
					[a.key, "1"],
					[b.key, "2"],
				])
			)
		).toBe("KEY is listed twice.");
		expect(reason({ name: "X", program: "x", env: [a] })).toBe("Type a value for KEY.");
	});
});

describe("what an agent can do", () => {
	it("reads the last start, or the problem it hit", () => {
		expect(probeLine(GEMINI, GEMINI_ACCOUNT)).toEqual({
			text: "Takes images, starts fresh on restart, has a plan mode, runs its default model.",
			error: false,
		});
		expect(probeLine(GEMINI, undefined).text).toBe("Not checked yet. Saving it checks it.");
		expect(probeLine({ ...GEMINI, problem: "found no gemini on PATH" }, undefined)).toEqual({
			text: "Found no gemini on PATH.",
			error: true,
		});
	});

	it("keeps history and takes images only when it said so", () => {
		expect(keepsHistory(GEMINI_ACCOUNT)).toBe(false);
		expect(takesImages(GEMINI_ACCOUNT)).toBe(true);
		const bare = { ...GEMINI_ACCOUNT, acp: null };
		expect(takesImages(bare)).toBe(false);
		expect(keepsHistory(account({ id: "a-claude" }))).toBe(true);
		expect(takesImages(account({ id: "a-claude" }))).toBe(true);
	});

	it("never shows Auto, and shows Plan only with a plan mode", () => {
		expect(modeShown("acp", "auto", { plan: true })).toBe(false);
		expect(modeShown("acp", "plan", { plan: false })).toBe(false);
		expect(vendorHasMode("acp", "plan", { plan: true })).toBe(true);
		expect(modeShown("codex", "plan")).toBe(true);
		expect(vendorHasMode("codex", "plan")).toBe(false);
		expect(modeShown("claude", "auto")).toBe(true);
	});

	it("starts in Accept edits where the device would start in Auto", () => {
		const view = viewOf(true);
		expect(startingMode(view, "dev-mac", "acp")).toBe("accept_edits");
		expect(startingMode(view, "dev-mac", "claude")).toBe("auto");
	});
});

describe("the sign-in card", () => {
	it("shows for the error maxd gives an agent that needs a sign-in", () => {
		const stopped = (reason: string) =>
			agent({ id: "a1", vendor: "acp", activity: { state: "error", reason } });
		expect(needsSignIn(stopped("Gemini needs a sign-in."), GEMINI_ACCOUNT)).toBe(true);
		expect(needsSignIn(stopped("exited with status 1"), GEMINI_ACCOUNT)).toBe(false);
		expect(needsSignIn(stopped("Gemini needs a sign-in."), undefined)).toBe(false);
	});

	it("copies the program and its arguments as one shell line", () => {
		expect(signInCommand(GEMINI)).toBe("gemini --experimental-acp");
		expect(signInCommand({ ...GEMINI, program: "/opt/my agent/run", args: ["it's"] })).toBe(
			`'/opt/my agent/run' 'it'\\''s'`
		);
	});
});
