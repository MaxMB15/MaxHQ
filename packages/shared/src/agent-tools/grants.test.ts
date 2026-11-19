// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AccessKey, Permission } from "../bindings";
import { NO_LIMITS } from "../data/source";
import { agent, device, entities, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import { agentGrants, agentRows, keyGrants, keyRows } from "./grants";

const permission = (id: string, holder: Permission["holder"], home = "mac"): Permission => ({
	id,
	holder,
	home,
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

describe("the page's rows", () => {
	const view = applySnapshot({
		seq: 1,
		entities: entities({
			devices: [device({ id: "mac", name: "Mac" }), device({ id: "devbox", name: "devbox" })],
			agents: [agent({ id: "a1", title: "Review" }), agent({ id: "a2", title: "" })],
		}),
	});

	it("names each agent, where its permission lives and its limits", () => {
		const rows = agentRows(
			view,
			[
				permission("p-gone", { kind: "agent", agent: "a9" }),
				permission("p-a1", { kind: "agent", agent: "a1" }, "devbox"),
				permission("p-a2", { kind: "agent", agent: "a2" }),
				permission("p-key", { kind: "key", key: "bot" }),
			],
			NOW
		);
		expect(rows).toEqual([
			{ permission: "p-a2", title: "This agent", line: "On Mac. No limits" },
			{ permission: "p-a1", title: "Review", line: "On devbox. No limits" },
			{ permission: "p-gone", title: "This agent is gone", line: "On Mac. No limits" },
		]);
	});

	const bot: AccessKey = {
		id: "bot",
		device: "devbox",
		name: "slack-bot",
		path: "/home/me/.maxhq/keys/slack bot",
		permission: "p-bot",
		command: { command: "maxd", args: ["mcp", "--key", "/home/me/.maxhq/keys/slack bot"] },
	};

	it("gives each key its permission's limits and the command that uses it", () => {
		const held = {
			...permission("p-bot", { kind: "key", key: "bot" }),
			limits: { ...NO_LIMITS, max_running: 2 },
		};
		expect(keyRows(view, [bot], [held], NOW)).toEqual([
			{
				id: "bot",
				permission: "p-bot",
				name: "slack-bot",
				line: "On devbox. 2 at once",
				path: "/home/me/.maxhq/keys/slack bot",
				command: "maxd mcp --key '/home/me/.maxhq/keys/slack bot'",
			},
		]);
	});

	it("says only where a key is while its permission isn't read", () => {
		expect(keyRows(view, [bot], [], NOW)[0]?.line).toBe("On devbox");
	});
});
