// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Account, Vendor } from "../bindings";
import { account, agent } from "../test/build";

import {
	accountKind,
	accountKindName,
	accountMark,
	AGENT_KINDS,
	agentKind,
	agentKindName,
	agentMark,
	cloudServerOf,
	isLocalAccount,
	isLocalAgent,
	kindName,
	ownKind,
	type ServerName,
} from "./agent-kind";

const local = account({
	id: "acct-local",
	vendor: "claude",
	login: "model_server",
	plan: null,
	model_server: "srv-1",
});
const max = account({ id: "acct-max", vendor: "claude", plan: "max" });
const codex = account({ id: "acct-codex", vendor: "codex", plan: "pro" });

const accounts: Account[] = [local, max, codex];
const table = new Map(accounts.map((a) => [a.id, a]));

const speed = { prompt_per_second: 900, decode_per_second: 60, context_used: 1, context_size: 2 };

describe("agentKind", () => {
	it("never calls a local agent Claude, with its account in a list or a table", () => {
		const running = agent({ id: "a", vendor: "claude", account: local.id });
		for (const lookup of [accounts, table]) {
			expect(agentKind(running, lookup)).toBe("local");
			expect(isLocalAgent(running, lookup)).toBe(true);
			expect(agentKindName(running, lookup)).toBe("Local");
			expect(agentKindName(running, lookup)).not.toMatch(/Claude/);
		}
	});

	it("calls an agent Local once its model server reports on it, even without its account", () => {
		const running = agent({ id: "a", vendor: "claude", account: null, local: speed });
		expect(agentKind(running, [])).toBe("local");
		expect(agentKind({ ...running, account: "acct-gone" }, table)).toBe("local");
		expect(ownKind(running)).toBe("local");
	});

	it("keeps every other agent's vendor", () => {
		expect(agentKind(agent({ id: "a", vendor: "claude", account: max.id }), table)).toBe(
			"claude"
		);
		expect(agentKind(agent({ id: "b", vendor: "codex", account: codex.id }), table)).toBe(
			"codex"
		);
		expect(agentKind(agent({ id: "c", vendor: "cursor", account: null }), table)).toBe(
			"cursor"
		);
		expect(agentKind(agent({ id: "d", vendor: "claude", account: "acct-gone" }), table)).toBe(
			"claude"
		);
		expect(isLocalAgent(agent({ id: "e", vendor: "claude", account: max.id }), accounts)).toBe(
			false
		);
	});
});

describe("cloud services", () => {
	const cloud = account({
		id: "acct-cloud",
		vendor: "claude",
		login: "model_server",
		plan: null,
		model_server: "srv-cloud",
	});
	const servers = new Map<string, ServerName>([
		["srv-1", { label: "Ollama on devbox", cloud: false }],
		["srv-cloud", { label: "OpenRouter", cloud: true }],
	]);
	const lookup = [...accounts, cloud];
	const onCloud = agent({ id: "a", vendor: "claude", account: cloud.id });

	it("names an agent on a cloud service after it, with a cloud, never Claude", () => {
		expect(cloudServerOf(cloud, servers)?.label).toBe("OpenRouter");
		expect(accountKindName(cloud, servers)).toBe("OpenRouter");
		expect(accountMark(cloud, servers)).toBe("cloud");
		expect(agentKindName(onCloud, lookup, servers)).toBe("OpenRouter");
		expect(agentMark(onCloud, lookup, servers)).toBe("cloud");
		// The logic still counts it as Local.
		expect(agentKind(onCloud, lookup)).toBe("local");
	});

	it("keeps a server on your own hardware, and every vendor, as they were", () => {
		const onHardware = agent({ id: "b", vendor: "claude", account: local.id });
		expect(cloudServerOf(local, servers)).toBeUndefined();
		expect(accountKindName(local, servers)).toBe("Local");
		expect(agentKindName(onHardware, lookup, servers)).toBe("Local");
		expect(agentMark(onHardware, lookup, servers)).toBe("local");
		expect(accountKindName(max, servers)).toBe("Claude");
		expect(accountMark(codex, servers)).toBe("codex");
	});

	it("shows a server from a maxd older than cloud servers as Local", () => {
		const older = new Map<string, ServerName>([["srv-cloud", { label: "OpenRouter" }]]);
		expect(cloudServerOf(cloud, older)).toBeUndefined();
		expect(agentKindName(onCloud, lookup, older)).toBe("Local");
		expect(agentMark(onCloud, lookup, older)).toBe("local");
		// Without the servers at hand, too.
		expect(agentKindName(onCloud, lookup)).toBe("Local");
	});
});

describe("accountKind", () => {
	it("shows a model server's account as Local and the rest as their vendor", () => {
		expect(isLocalAccount(local)).toBe(true);
		expect(isLocalAccount(max)).toBe(false);
		expect(isLocalAccount(null)).toBe(false);
		expect(accountKind(local)).toBe("local");
		expect(accountKind(max)).toBe("claude");
		expect(accountKind(codex)).toBe("codex");
	});
});

describe("kindName", () => {
	it("names each kind, with Codex as Codex", () => {
		const vendors: Record<Vendor, string> = {
			claude: "Claude",
			codex: "Codex",
			cursor: "Cursor",
			acp: "ACP agent",
		};
		for (const [vendor, name] of Object.entries(vendors)) {
			expect(kindName(vendor as Vendor)).toBe(name);
		}
		expect(kindName("local")).toBe("Local");
		expect(AGENT_KINDS).toContain("local");
	});
});
