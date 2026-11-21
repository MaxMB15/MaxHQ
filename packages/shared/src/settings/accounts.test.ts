// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, ToolInfo } from "../bindings";
import { account, device, entities, link, NOW } from "../test/build";
import { githubApp, githubUser } from "../test/pulls";
import { applySnapshot } from "../view/replica";

import { accountDevices, accountRows } from "./accounts";

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

const view = (more: Entity[] = []) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				links: [
					link({ id: "devbox", device: "dev-devbox" }),
					link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
					link({ id: "alpha", device: "dev-alpha" }),
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
				["github", "GitHub", null, ["signed_in", "signed_out", "signed_in"], null],
			]
		);
	});

	it("reads a device that hasn't reported its CLIs as not signed in", () => {
		const v = view();
		const [, , cursor] = accountRows(v, [{ device: "dev-devbox", name: "devbox" }]);
		expect(cursor?.cells).toEqual(["signed_out"]);
	});
});
