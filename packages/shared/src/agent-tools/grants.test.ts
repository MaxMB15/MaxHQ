// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AccessKey, Permission } from "../bindings";
import { NO_LIMITS } from "../data/source";
import { agent, entities } from "../test/build";
import { applySnapshot } from "../view/replica";
import { agentGrants, keyGrants } from "./grants";

const permission = (id: string, holder: Permission["holder"]): Permission => ({
	id,
	holder,
	home: "mac",
	devices: ["mac"],
	vendors: ["claude"],
	accounts: null,
	places: null,
	depth: 3,
	limits: NO_LIMITS,
});

const key = (id: string, held: string): AccessKey => ({
	id,
	device: "mac",
	name: id,
	path: `/keys/${id}`,
	permission: held,
	command: { command: "maxd", args: ["mcp"] },
});

describe("access keys", () => {
	it("pairs each key with the permission it holds, in the keys' order", () => {
		const held = permission("p-bot", { kind: "key", key: "bot" });
		const grants = keyGrants([key("bot", "p-bot"), key("old", "p-gone")], [held]);
		expect(grants.map((g) => [g.key.id, g.permission?.id])).toEqual([
			["bot", "p-bot"],
			["old", undefined],
		]);
	});
});

describe("agents' permissions", () => {
	const view = applySnapshot({
		seq: 1,
		entities: entities({
			agents: [agent({ id: "a1", title: "Review" }), agent({ id: "a2", title: "Fix tests" })],
		}),
	});

	it("lists only agents' permissions, by title, with gone agents last", () => {
		const grants = agentGrants(view, [
			permission("p-gone", { kind: "agent", agent: "a9" }),
			permission("p-key", { kind: "key", key: "bot" }),
			permission("p-task", { kind: "task", task: "t1" }),
			permission("p-a1", { kind: "agent", agent: "a1" }),
			permission("p-a2", { kind: "agent", agent: "a2" }),
		]);
		expect(grants.map((g) => [g.permission.id, g.agent?.title])).toEqual([
			["p-a2", "Fix tests"],
			["p-a1", "Review"],
			["p-gone", undefined],
		]);
	});
});
