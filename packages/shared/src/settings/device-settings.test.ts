// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { EFFORT_SINCE, type Entity, type Tool } from "../bindings";
import { entities, link } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	deviceSettingsOf,
	KEEP_AWAKE_OPTIONS,
	nameOptions,
	PLACEMENT_OPTIONS,
	sameDeviceFields,
	settingsWhy,
} from "./device-settings";

const NEW_SETTINGS = {
	entity: "device_settings",
	device: "dev-new",
	default_permission: "auto",
	workspace_network: null,
	workspace_pool: null,
	continue_claude_at_limit: false,
	idle_release_minutes: 60,
	memory_items: 256,
	claude_setup_token: false,
} as Entity;

const view = applySnapshot({
	seq: 1,
	entities: [
		...entities({
			links: [
				link({
					id: "new",
					protocol: EFFORT_SINCE,
					capabilities: { transfers: true, tunnels: true, workspaces: true, inbox: true },
				}),
				link({ id: "old", protocol: null }),
			],
		}),
		NEW_SETTINGS,
	],
});

describe("sameDeviceFields", () => {
	const tools: Tool[] = ["claude", "codex"];
	const fields = { ...deviceSettingsOf(view, "dev-new"), tool_updates: tools };

	it("takes tool updates in any order", () => {
		expect(sameDeviceFields(fields, { ...fields, tool_updates: [...tools].reverse() })).toBe(
			true
		);
	});

	it("tells any change apart", () => {
		expect(sameDeviceFields(fields, { ...fields, auto_settle_days: 9 })).toBe(false);
		expect(sameDeviceFields(fields, { ...fields, network: { port: 7000 } })).toBe(false);
		expect(sameDeviceFields(fields, { ...fields, tool_updates: tools.slice(0, 1) })).toBe(
			false
		);
		expect(sameDeviceFields(fields, { ...fields, codex_effort: "high" })).toBe(false);
	});
});

describe("choices", () => {
	it("lists placement and keep-awake in the desktop's order", () => {
		expect(PLACEMENT_OPTIONS.map((o) => o.value)).toEqual(["prefer", "normal"]);
		expect(PLACEMENT_OPTIONS[0]?.detail).toBe("Counts twice when MaxHQ weighs devices");
		expect(KEEP_AWAKE_OPTIONS.map((o) => o.value)).toEqual([
			"charger",
			"always",
			"working",
			"off",
		]);
		expect(KEEP_AWAKE_OPTIONS[0]?.label).toBe("On the charger");
	});

	it("labels names as themselves", () => {
		expect(nameOptions(["incusbr0", "guarded"])).toEqual([
			{ value: "incusbr0", label: "incusbr0" },
			{ value: "guarded", label: "guarded" },
		]);
	});
});

describe("settingsWhy", () => {
	it("says to connect first when the device doesn't answer", () => {
		const why = settingsWhy(view, "dev-new", "devbox", false);
		const offline = "Connect to devbox to change it.";
		expect(why).toEqual({
			offline,
			inbox: offline,
			limits: offline,
			idle: offline,
			memory: offline,
			tokens: offline,
			effort: offline,
		});
	});

	it("lets every part change on a live, current maxd", () => {
		expect(settingsWhy(view, "dev-new", "devbox", true)).toEqual({
			offline: null,
			inbox: null,
			limits: null,
			idle: null,
			memory: null,
			tokens: null,
			effort: null,
		});
	});

	it("asks to update a maxd from before each part", () => {
		const why = settingsWhy(view, "dev-old", "oldbox", true);
		expect(why.offline).toBeNull();
		expect(why.inbox).toBe("oldbox's maxd is too old for these. Update it first.");
		expect(why.memory).toBe("oldbox's maxd is too old for these. Update it first.");
		expect(why.limits).toBe("oldbox's maxd is too old for this. Update it first.");
		expect(why.idle).toBe("oldbox's maxd is too old for this. Update it first.");
		expect(why.effort).toBe("oldbox's maxd is too old for this. Update it first.");
		expect(why.tokens).toBe(
			"oldbox's maxd is too old for this, and takes setup-tokens. Update it first."
		);
	});
});
