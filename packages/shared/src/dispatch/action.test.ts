// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AdminAction, Entity, Plan } from "../bindings";
import { agent, device, entities, project } from "../test/build";
import { applySnapshot } from "../view/replica";

import { actionCard, actsDirectly, adminPhrase, settingLines, type ActionItem } from "./action";
import { itemLines, itemPreview, outcomeLine, planSummary } from "./plan";

const view = (acts?: boolean) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [
					device({ id: "dev-mac", name: "Home Mac", os: "macos" }),
					device({ id: "dev-box", name: "devbox" }),
				],
				projects: [project("p-comet", "Comet")],
				agents: [agent({ id: "a-login", title: "Fix login" })],
			}),
			{
				entity: "dispatcher",
				device: "dev-mac",
				workspace: "ws-dispatcher",
				agent: "a-dispatcher",
				plan: null,
				undo: ["key-ready"],
			},
			...(acts === undefined
				? []
				: [
						{
							entity: "device_settings",
							device: "dev-mac",
							default_permission: "auto",
							workspace_network: null,
							workspace_pool: null,
							dispatcher_acts: acts,
						} satisfies Entity,
					]),
		],
	});

const item = (over: Partial<ActionItem> & { action: AdminAction }): ActionItem => ({
	id: "item-1",
	agent: "a-dispatcher",
	turn: null,
	parent: null,
	ordinal: 3,
	status: "completed",
	started_at: null,
	completed_at: null,
	kind: "dispatcher_action",
	device: "dev-mac",
	target: "",
	undo: "key-ready",
	...over,
});

const rename: AdminAction = { action: "rename_agent", agent: "a-login", title: "Auth fix" };

describe("a change the dispatcher made at once", () => {
	it("says what it did, by the name it had then", () => {
		const card = actionCard(view(), item({ action: rename, target: "Fix login" }));
		expect(card).toEqual({
			line: "Renamed Fix login to Auth fix",
			elsewhere: null,
			home: "dev-mac",
			undo: "ready",
		});
	});

	it("falls back to the name it has now", () => {
		const hide: AdminAction = { action: "hide_project", project: "p-comet", hidden: true };
		expect(actionCard(view(), item({ action: hide, undo: null })).line).toBe(
			"Hid Comet from the sidebar"
		);
	});

	it("greys Undo out once maxd no longer holds it, and has none for a change without one", () => {
		expect(actionCard(view(), item({ action: rename, undo: "key-old" })).undo).toBe("gone");
		expect(actionCard(view(), item({ action: rename, undo: null })).undo).toBeNull();
	});

	it("names the device it changed when that isn't the dispatcher's", () => {
		const settled = item({
			device: "dev-box",
			action: { action: "set_inbox", agent: "a-box", change: { change: "settle" } },
			target: "Nightly build",
		});
		expect(actionCard(view(), settled)).toMatchObject({
			line: "Settled Nightly build",
			elsewhere: "devbox",
			home: "dev-mac",
		});
	});
});

describe("a change that waits in a plan", () => {
	const settings: AdminAction = {
		action: "change_settings",
		changes: [
			{ field: "default_permission", value: "full_access" },
			{ field: "snooze_at_limit", value: true },
			{ field: "claude_effort", value: null },
		],
	};

	it("phrases it for the plan's summary", () => {
		expect(adminPhrase(view(), "dev-mac", rename)).toBe("renaming Fix login to Auth fix");
		expect(adminPhrase(view(), "dev-box", settings)).toBe("changing 3 settings on devbox");
	});

	it("lists each setting it sets", () => {
		expect(settingLines(settings)).toEqual([
			"default permission: Full access",
			"snooze at limit: On",
			"claude effort: Default",
		]);
		expect(settingLines(rename)).toEqual([]);
	});

	it("shows in the plan as a change that sends nothing", () => {
		const plan: Plan = {
			id: 2,
			items: [
				{
					reason: "Deleting can't be undone",
					step: {
						type: "admin",
						device: "dev-mac",
						action: { action: "delete_agent", agent: "a-login" },
					},
				},
			],
		};
		expect(planSummary(view(), plan)).toBe("1 item: deleting Fix login");
		expect(itemPreview(plan.items[0]!)).toBeNull();
		expect(itemLines(view(), plan, 0)).toEqual([
			{ label: "Device", value: "Home Mac" },
			{ label: "Change", value: "Deleting Fix login" },
		]);
		expect(outcomeLine({ type: "done" })).toBe("Done");
	});
});

describe("the Act directly switch", () => {
	it("reads the device's setting, or null when its maxd predates it", () => {
		expect(actsDirectly(view(true), "dev-mac")).toBe(true);
		expect(actsDirectly(view(false), "dev-mac")).toBe(false);
		expect(actsDirectly(view(), "dev-mac")).toBeNull();
	});
});
