// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Account } from "../bindings";
import { account, MIN, NOW } from "../test/build";

import { subscriptions } from "./subscriptions";
import { meterRows } from "./usage";

const reading = (percent: number, checked_at = NOW): Account["usage"] => ({
	checked_at,
	source: "claude_cache",
	windows: [{ scope: null, window_minutes: 300, used_percent: percent, resets_at: null }],
});

/** The Mac's own Claude Max login, read a minute ago. */
const mac = account({ id: "mac", usage: reading(22, NOW - MIN) });

/** A container's setup-token minted from the Mac login, which reads nothing. */
const token = account({
	id: "token",
	device: "devbox",
	login: "setup_token",
	origin: "mac",
	email: null,
	plan: null,
	config_dir: "/home/agent/.claude-token",
});

const ids = (accounts: readonly Account[]) =>
	subscriptions(accounts).map((p) => [p.account.id, p.logins.map((a) => a.id)]);

describe("subscriptions", () => {
	it("shows a token with the login it came from", () => {
		const [plan, ...rest] = subscriptions([mac, token]);
		expect(rest).toEqual([]);
		expect(plan!.account).toEqual(mac);
		expect(plan!.logins).toEqual([mac, token]);
		expect(plan!.borrowed).toBe(false);
	});

	it("shows a Codex sign-in with the Mac login on the same email", () => {
		const codex = account({ id: "codex", vendor: "codex", plan: "pro", usage: reading(10) });
		const signIn = account({
			id: "sign-in",
			vendor: "codex",
			device: "devbox",
			login: "chat_gpt_sign_in",
			email: "Someone@Example.com",
			plan: null,
		});
		expect(ids([codex, signIn])).toEqual([["codex", ["codex", "sign-in"]]]);
	});

	it("shows the same login on two devices once, with the newest reading", () => {
		const server = account({
			id: "server",
			device: "devbox",
			plan: null,
			usage: reading(80, NOW - 60 * MIN),
		});
		const newer = account({ ...server, id: "newer", usage: reading(5, NOW) });
		expect(ids([mac, server])).toEqual([["mac", ["mac", "server"]]]);
		const [plan] = subscriptions([mac, newer]);
		expect(plan!.account.id).toBe("newer");
		expect(plan!.account.plan).toBe("max");
	});

	it("keeps logins on other emails or organizations apart", () => {
		const other = account({ id: "other", email: "other@example.com" });
		const team = account({ id: "team", organization: "Team" });
		expect(ids([mac, other, team])).toEqual([
			["mac", ["mac"]],
			["other", ["other"]],
			["team", ["team"]],
		]);
	});

	it("leaves out a login that knows nothing once the vendor has one that does", () => {
		const blank = account({ id: "blank", device: "devbox", email: null, plan: null });
		const failed = account({
			...blank,
			id: "failed",
			usage_failure: { at: NOW, reason: "claude not found" },
		});
		expect(ids([mac, blank])).toEqual([["mac", ["mac"]]]);
		expect(ids([blank, failed, mac])).toEqual([
			["failed", ["failed"]],
			["mac", ["mac"]],
		]);
		const codex = account({ id: "codex", vendor: "codex", email: null, plan: null });
		expect(ids([mac, codex])).toEqual([
			["mac", ["mac"]],
			["codex", ["codex"]],
		]);
	});

	it("keeps a token whose login isn't in sight, marked as borrowed", () => {
		const [plan] = subscriptions([token]);
		expect(plan!.account.id).toBe("token");
		expect(plan!.borrowed).toBe(true);
		expect(ids([account({ ...token, id: "lost", origin: "gone" }), mac])).toHaveLength(2);
	});

	it("never joins ACP agents", () => {
		const gemini = account({ id: "gemini", vendor: "acp", login: "acp_command" });
		const other = account({ ...gemini, id: "other" });
		expect(ids([gemini, other])).toEqual([
			["gemini", ["gemini"]],
			["other", ["other"]],
		]);
	});

	it("leaves out a local model's login, which has no plan or usage", () => {
		const local = account({
			id: "local",
			login: "model_server",
			model_server: "srv",
			usage: reading(10),
		});
		expect(ids([mac, local])).toEqual([["mac", ["mac"]]]);
		expect(meterRows([local])).toEqual([]);
	});
});

describe("meterRows", () => {
	it("reads a plan from its newest reading, not an old one on another device", () => {
		const stale = account({
			id: "stale",
			device: "devbox",
			usage: reading(80, NOW - 60 * MIN),
		});
		expect(meterRows([mac, stale, token])[0]?.percent).toBe(22);
	});
});
