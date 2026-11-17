// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AccessKey, Permission } from "@maxhq/shared/bindings";
import { NO_LIMITS } from "@maxhq/shared/data/source";
import { agent, device, entities, NOW } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import { agentRows, keyRows } from "./agent-tools";

const view = applySnapshot({
	seq: 1,
	entities: entities({
		devices: [device({ id: "mac", name: "Mac" }), device({ id: "devbox", name: "devbox" })],
		agents: [agent({ id: "a1", title: "Review" }), agent({ id: "a2", title: "" })],
	}),
});

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

describe("agents allowed to run agents", () => {
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
});

describe("access keys", () => {
	const key: AccessKey = {
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
			limits: {
				...NO_LIMITS,
				max_running: 2,
			},
		};
		expect(keyRows(view, [key], [held], NOW)).toEqual([
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
		expect(keyRows(view, [key], [], NOW)[0]?.line).toBe("On devbox");
	});
});
