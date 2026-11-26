// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, ModelServer, ToolInfo } from "../bindings";
import { OPENROUTER_URL } from "../local-models/openrouter";
import { account, device, entities, link, NOW } from "../test/build";
import { githubApp, githubUser } from "../test/pulls";
import { applySnapshot } from "../view/replica";

import { accountCellText, accountDevices, accountRows } from "./accounts";

const tool = (over: Partial<ToolInfo>): ToolInfo => ({
	kind: { type: "agent", tool: "claude" },
	place: { type: "host" },
	path: "/usr/local/bin/claude",
	version: "2.1.40",
	latest: null,
	owner: "user",
	method: "native",
	update: { type: "needs_tick" },
	state: { type: "idle" },
	...over,
});

/** An OpenRouter server on `device`, with a key whose limit has $7.42 left. */
const openRouter = (device: string, over: Partial<ModelServer> = {}): Entity => ({
	entity: "model_server",
	id: `srv-${device}`,
	device,
	kind: { type: "external", flavor: "other" },
	label: "OpenRouter",
	base_url: OPENROUTER_URL,
	token: true,
	cloud: true,
	probe: null,
	models: [],
	credits: { at: NOW, used_cents: 258, limit_cents: 1_000 },
	...over,
});

/** The links with local models on alpha and devbox, as `maxd` reports them. */
const RUNS_MODELS = { transfers: true, tunnels: true, workspaces: true, local_models: true };

const view = (more: Entity[] = []) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				links: [
					link({ id: "devbox", device: "dev-devbox", capabilities: RUNS_MODELS }),
					link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
					link({ id: "alpha", device: "dev-alpha", capabilities: RUNS_MODELS }),
					link({
						id: "claim",
						device: null,
						claim: { device: "dev-mac" } as never,
					}),
				],
				devices: [
					device({ id: "dev-mac", name: "Mac", os: "macos" }),
					device({ id: "dev-devbox", name: "devbox" }),
					device({ id: "dev-alpha", name: "alpha" }),
				],
				accounts: [
					account({ id: "mac-claude", plan: "max" }),
					account({ id: "box-claude", device: "dev-devbox", plan: null }),
					account({ id: "mac-codex", vendor: "codex", label: "Codex", plan: "pro" }),
					account({
						id: "box-codex",
						device: "dev-devbox",
						vendor: "codex",
						label: "Codex",
						usage_failure: { at: NOW, reason: "Logged out of Codex" } as never,
					}),
				],
			}),
			{
				entity: "device_tools",
				device: "dev-alpha",
				tools: [
					tool({ kind: { type: "agent", tool: "claude" } }),
					tool({ kind: { type: "agent", tool: "cursor" }, path: null, version: null }),
				],
			},
			...more,
		],
	});

describe("the accounts table", () => {
	it("has a column for each linked device, this Mac first", () => {
		expect(accountDevices(view())).toEqual([
			{ device: "dev-mac", name: "Mac" },
			{ device: "dev-alpha", name: "alpha" },
			{ device: "dev-devbox", name: "devbox" },
		]);
	});

	it("says whether each device is signed in, and offers the first that isn't", () => {
		const v = view([githubUser("dev-mac"), githubApp("dev-devbox")]);
		const rows = accountRows(v, accountDevices(v));
		expect(rows.map((r) => [r.login, r.name, r.plan, r.cells, r.signIn?.name ?? null])).toEqual(
			[
				["claude", "Claude", "Max", ["signed_in", "signed_out", "signed_in"], "alpha"],
				["codex", "Codex", "Pro", ["signed_in", "not_installed", "signed_out"], "devbox"],
				["cursor", "Cursor", null, ["signed_out", "not_installed", "signed_out"], "Mac"],
				[
					"openrouter",
					"OpenRouter",
					null,
					["not_set_up", "not_set_up", "not_set_up"],
					"alpha",
				],
				["github", "GitHub", null, ["signed_in", "signed_out", "signed_in"], null],
			]
		);
		expect(rows.find((r) => r.login === "openrouter")?.notes).toEqual([null, null, null]);
		expect(rows.every((r) => r.notes.length === r.cells.length)).toBe(true);
	});

	it("reads a device with an OpenRouter key as signed in, with its credits", () => {
		const v = view([openRouter("dev-devbox")]);
		const row = accountRows(v, accountDevices(v)).find((r) => r.login === "openrouter")!;
		expect(row.cells).toEqual(["not_set_up", "not_set_up", "signed_in"]);
		expect(row.notes).toEqual([null, null, "$7.42 left"]);
		expect(row.cells.map((cell, at) => accountCellText(cell, row.notes[at]))).toEqual([
			"Not set up",
			"Not set up",
			"Signed in · $7.42 left",
		]);
		// Another device needs no key of its own, so the row manages.
		expect(row.signIn).toBeNull();
	});

	it("reads an OpenRouter server without a key as not signed in, and offers it", () => {
		const v = view([openRouter("dev-mac"), openRouter("dev-devbox", { token: false })]);
		const row = accountRows(v, accountDevices(v)).find((r) => r.login === "openrouter")!;
		expect(row.cells).toEqual(["signed_in", "not_set_up", "signed_out"]);
		expect(row.signIn?.name).toBe("devbox");
	});

	it("leaves out OpenRouter's credits before the first read", () => {
		const v = view([openRouter("dev-alpha", { credits: null })]);
		const row = accountRows(v, accountDevices(v)).find((r) => r.login === "openrouter")!;
		expect(row.cells).toEqual(["not_set_up", "signed_in", "not_set_up"]);
		expect(row.notes).toEqual([null, null, null]);
	});

	it("reads a device that hasn't reported its CLIs as not signed in", () => {
		const v = view();
		const [, , cursor] = accountRows(v, [{ device: "dev-devbox", name: "devbox" }]);
		expect(cursor?.cells).toEqual(["signed_out"]);
	});
});
