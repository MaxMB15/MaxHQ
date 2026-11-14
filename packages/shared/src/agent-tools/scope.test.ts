// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Permission } from "../bindings";
import { AGENT_DEFAULTS, NO_LIMITS } from "../data/source";
import { account, device, entities, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	accountChoices,
	commandLine,
	heldBy,
	keyNameError,
	limitsDraft,
	limitsFrom,
	limitsLine,
	offeredOnly,
	permissionFrom,
	placeChoices,
	scopeDraft,
	shellQuote,
} from "./scope";

const NOW = new Date(2026, 9, 31, 12, 0).getTime();
const DAY = 24 * 60 * 60 * 1000;
const who = { id: "perm-1", holder: { kind: "agent", agent: "a1" } as const, home: "mac" };

const permission: Permission = {
	id: "perm-1",
	holder: { kind: "agent", agent: "a1" },
	home: "mac",
	devices: ["devbox"],
	vendors: ["codex"],
	accounts: ["acc-1"],
	places: [{ type: "workspace", workspace: "ws-1" }],
	depth: 2,
	limits: { ...NO_LIMITS, max_running: 2, expires_at: NOW + 3 * DAY },
	test_kinds: ["ios_simulator"],
	leave_container: "never",
};

describe("permission scope", () => {
	it("starts a new permission over everything with the defaults", () => {
		const draft = scopeDraft(undefined, AGENT_DEFAULTS, ["mac", "devbox"], NOW);
		expect(draft).toEqual({
			devices: ["mac", "devbox"],
			vendors: ["claude", "codex"],
			accounts: null,
			places: null,
			depth: "3",
			limits: { started: "", running: "", usage: "", days: "" },
			testKinds: [],
			leaveContainer: "ask",
		});
		expect(permissionFrom(draft, who, NOW)).toEqual({
			ok: {
				...who,
				devices: ["mac", "devbox"],
				vendors: ["claude", "codex"],
				accounts: null,
				places: null,
				depth: 3,
				limits: NO_LIMITS,
				test_kinds: [],
				leave_container: "ask",
			},
		});
	});

	it("reads a permission from before test hosts as leasing none", () => {
		const old: Permission = { ...permission };
		delete old.test_kinds;
		expect(scopeDraft(old, AGENT_DEFAULTS, ["mac"], NOW).testKinds).toEqual([]);
	});

	it("reads a permission from before the container setting as ask", () => {
		const old: Permission = { ...permission };
		delete old.leave_container;
		expect(scopeDraft(old, AGENT_DEFAULTS, ["mac"], NOW).leaveContainer).toBe("ask");
	});

	it("keeps each container setting", () => {
		for (const leave of ["ask", "allow", "never"] as const) {
			const held: Permission = { ...permission, leave_container: leave };
			const draft = scopeDraft(held, AGENT_DEFAULTS, ["mac"], NOW);
			expect(draft.leaveContainer).toBe(leave);
			expect(permissionFrom(draft, who, NOW)).toEqual({ ok: held });
		}
	});

	it("reads a permission back the way it was", () => {
		const draft = scopeDraft(permission, AGENT_DEFAULTS, ["mac"], NOW);
		expect(draft.limits).toEqual({ started: "", running: "2", usage: "", days: "3" });
		expect(draft.testKinds).toEqual(["ios_simulator"]);
		expect(permissionFrom(draft, who, NOW)).toEqual({ ok: permission });
	});

	it("says why a form can't be saved", () => {
		const draft = scopeDraft(undefined, AGENT_DEFAULTS, ["mac"], NOW);
		const error = (over: Partial<typeof draft>) => {
			const parsed = permissionFrom({ ...draft, ...over }, who, NOW);
			return "error" in parsed ? parsed.error : undefined;
		};
		expect(error({ devices: [] })).toBe("Pick at least one device.");
		expect(error({ vendors: [] })).toBe("Pick at least one vendor.");
		expect(error({ accounts: [] })).toBe("Pick at least one account, or allow any.");
		expect(error({ places: [] })).toBe("Pick at least one place, or allow any.");
		expect(error({ depth: "0" })).toMatch(/^Depth must be/);
		expect(error({ depth: "2.5" })).toMatch(/^Depth must be/);
		expect(error({ limits: { ...draft.limits, usage: "120" } })).toMatch(
			/percent from 1 to 100/
		);
		expect(error({ limits: { ...draft.limits, running: "-1" } })).toMatch(/^Agents at once/);
	});

	it("turns the limits' fields into limits and back", () => {
		const limits = limitsFrom({ started: "5", running: " 2 ", usage: "80", days: "7" }, NOW);
		expect(limits).toEqual({
			ok: {
				max_started: 5,
				max_running: 2,
				usage_cap_percent: 80,
				expires_at: NOW + 7 * DAY,
			},
		});
		if ("ok" in limits) {
			expect(limitsDraft(limits.ok, NOW)).toEqual({
				started: "5",
				running: "2",
				usage: "80",
				days: "7",
			});
		}
	});

	it("finds the permission an agent holds", () => {
		const key: Permission = { ...permission, id: "perm-2", holder: { kind: "key", key: "a1" } };
		expect(heldBy([key, permission], "a1")).toBe(permission);
		expect(heldBy([key], "a1")).toBeUndefined();
	});

	it("says the limits in a few words", () => {
		expect(limitsLine(undefined, NOW)).toBe("No limits");
		expect(limitsLine(NO_LIMITS, NOW)).toBe("No limits");
		expect(
			limitsLine({ ...NO_LIMITS, max_started: 5, max_running: 2, usage_cap_percent: 80 }, NOW)
		).toBe("Up to 5 agents, 2 at once and stops at 80% usage");
		expect(limitsLine({ ...NO_LIMITS, expires_at: NOW + 60 * 60 * 1000 }, NOW)).toBe(
			"Ends 13:00"
		);
	});
});

describe("what a permission form offers", () => {
	const view = applySnapshot({
		seq: 1,
		entities: entities({
			devices: [device({ id: "mac", name: "Mac" }), device({ id: "devbox", name: "devbox" })],
			accounts: [
				account({ id: "claude-mac", device: "mac" }),
				account({ id: "codex-box", device: "devbox", vendor: "codex", plan: "plus" }),
			],
			workspaces: [
				workspace({ id: "w-mac", device: "mac" }),
				workspace({ id: "w-box", device: "devbox" }),
			],
		}),
	});
	const draft = {
		...scopeDraft(undefined, AGENT_DEFAULTS, ["mac"], NOW),
		vendors: ["claude" as const],
	};

	it("offers the accounts and workspaces on the devices and vendors picked", () => {
		expect(accountChoices(view, draft)).toEqual([
			{ value: "claude-mac", label: "Claude Max on Mac" },
		]);
		expect(placeChoices(view, draft)).toEqual([
			{ value: { type: "workspace", workspace: "w-mac" }, label: "w-mac on Mac" },
		]);
	});

	it("keeps a folder the permission names", () => {
		const places = [{ type: "folder" as const, path: "/home/dev/work" }];
		expect(placeChoices(view, { ...draft, places }).at(-1)).toEqual({
			value: places[0],
			label: "/home/dev/work",
		});
	});

	it("drops what it no longer offers", () => {
		const picked = {
			...draft,
			accounts: ["claude-mac", "codex-box"],
			places: [
				{ type: "workspace" as const, workspace: "w-mac" },
				{ type: "workspace" as const, workspace: "w-box" },
			],
		};
		expect(offeredOnly(view, picked)).toMatchObject({
			accounts: ["claude-mac"],
			places: [{ type: "workspace", workspace: "w-mac" }],
		});
		expect(offeredOnly(view, draft)).toMatchObject({ accounts: null, places: null });
	});
});

describe("key names", () => {
	it("takes letters, digits, dashes and underscores", () => {
		expect(keyNameError("slack-bot_2")).toBeNull();
		expect(keyNameError(" ")).toBe("Give the key a name.");
		expect(keyNameError("slack bot")).toMatch(/^Use only letters/);
	});
});

describe("MCP command line", () => {
	it("leaves plain words alone and quotes the rest", () => {
		expect(shellQuote("/home/dev/.local/bin/maxd")).toBe("/home/dev/.local/bin/maxd");
		expect(shellQuote("--key-file")).toBe("--key-file");
		expect(shellQuote("")).toBe("''");
		expect(shellQuote("Application Support")).toBe("'Application Support'");
		expect(shellQuote("it's")).toBe(`'it'\\''s'`);
		expect(shellQuote("$HOME")).toBe("'$HOME'");
	});

	it("joins a key's command into one line", () => {
		expect(
			commandLine({
				command: "/Users/dev/.local/bin/maxd",
				args: [
					"mcp",
					"--key-file",
					"/Users/dev/Library/Application Support/MaxHQ/keys/raycast.key",
				],
			})
		).toBe(
			"/Users/dev/.local/bin/maxd mcp --key-file '/Users/dev/Library/Application Support/MaxHQ/keys/raycast.key'"
		);
	});
});
