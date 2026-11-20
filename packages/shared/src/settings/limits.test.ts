// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Agent, Entity, Inbox } from "../bindings";
import { account, agent, entities, workspace } from "../test/build";
import { DEFAULT_INBOX } from "../view/inbox";
import { applySnapshot, type ViewState } from "../view/replica";

import { canContinueAtLimit, continuesAfterLimit, continuesAtLimit } from "./limits";

const settings = (device: string, claude?: boolean, codex?: boolean): Entity =>
	({
		entity: "device_settings",
		device,
		default_permission: "auto",
		workspace_network: null,
		workspace_pool: null,
		...(claude === undefined ? {} : { continue_claude_at_limit: claude }),
		...(codex === undefined ? {} : { continue_codex_at_limit: codex }),
	}) as Entity;

const viewOf = (agents: Agent[], extra: Entity[]): ViewState =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				agents,
				workspaces: [
					workspace({ id: "w1", device: "dev-mac" }),
					workspace({ id: "w2", device: "dev-old" }),
				],
			}),
			...extra,
		],
	});

const held = (over: Partial<Agent> & { id: string }): Agent => agent({ control: "held", ...over });

const own = (on: boolean): Inbox => ({ ...DEFAULT_INBOX, continue_at_limit: on });

describe("continuing after a usage limit", () => {
	const agents = [
		held({ id: "claude" }),
		held({ id: "codex", vendor: "codex" }),
		held({ id: "cursor", vendor: "cursor" }),
		agent({ id: "observed" }),
		held({ id: "sub", parent: "claude" }),
		held({ id: "old", workspace: "w2" }),
	];
	const view = viewOf(agents, [settings("dev-mac", true, false), settings("dev-old")]);
	const get = (id: string) => view.tables.agent.get(id)!;

	it("knows a maxd has it by its settings", () => {
		expect(continuesAtLimit(view, "dev-mac")).toBe(true);
		expect(continuesAtLimit(view, "dev-old")).toBe(false);
	});

	it("applies to held Claude Code and Codex leads only", () => {
		expect(canContinueAtLimit(view, get("claude"))).toBe(true);
		expect(canContinueAtLimit(view, get("codex"))).toBe(true);
		expect(canContinueAtLimit(view, get("cursor"))).toBe(false);
		expect(canContinueAtLimit(view, get("observed"))).toBe(false);
		expect(canContinueAtLimit(view, get("sub"))).toBe(false);
		expect(canContinueAtLimit(view, get("old"))).toBe(false);
	});

	it("never applies to a local agent, which has no limit", () => {
		const local = viewOf(
			[held({ id: "local", account: "l" })],
			[
				settings("dev-mac", true, true),
				{
					entity: "account",
					...account({ id: "l", login: "model_server", model_server: "srv" }),
				},
			]
		);
		const agent = local.tables.agent.get("local")!;
		expect(canContinueAtLimit(local, agent)).toBe(false);
		expect(continuesAfterLimit(local, agent)).toBe(false);
	});

	it("follows the device's default for the agent's vendor", () => {
		expect(continuesAfterLimit(view, get("claude"))).toBe(true);
		expect(continuesAfterLimit(view, get("codex"))).toBe(false);
		expect(continuesAfterLimit(view, get("cursor"))).toBe(false);
	});

	it("lets the agent's own switch beat the default", () => {
		const switched = viewOf(
			[
				held({ id: "claude", inbox: own(false) }),
				held({ id: "codex", vendor: "codex", inbox: own(true) }),
			],
			[settings("dev-mac", true, false)]
		);
		expect(continuesAfterLimit(switched, switched.tables.agent.get("claude")!)).toBe(false);
		expect(continuesAfterLimit(switched, switched.tables.agent.get("codex")!)).toBe(true);
	});
});
