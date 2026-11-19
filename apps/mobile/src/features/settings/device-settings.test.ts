// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity } from "@maxhq/shared/bindings";
import { deviceSettingsOf } from "@maxhq/shared/settings/device-settings";
import { device, entities, link } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import { draftOf, shownFields, someCards } from "./device-settings";

const view = applySnapshot({
	seq: 1,
	entities: [
		...entities({
			devices: [
				device({ id: "dev-mac", os: "macos" }),
				device({
					id: "dev-box",
					workspace_host: { networks: ["guarded"], pools: ["ssd"] },
				}),
			],
			links: [
				link({ id: "mac", route: { type: "local" } }),
				link({
					id: "box",
					capabilities: {
						transfers: true,
						tunnels: true,
						workspaces: true,
						skills: true,
					},
				}),
			],
		}),
		{
			entity: "network_access",
			device: "dev-box",
			on: false,
			port: 7717,
			hosts: [],
			pin: null,
			problem: null,
		} as Entity,
	],
});

describe("shownFields", () => {
	const current = deviceSettingsOf(view, "dev-box");
	const picked = { ...current, auto_settle_days: 9 };

	it("shows what you picked while the device still has what it had", () => {
		expect(shownFields({ fields: picked, from: current }, current)).toBe(picked);
		expect(shownFields(null, current)).toBe(current);
	});

	it("shows the device's once it changes", () => {
		const moved = { ...current, placement: "prefer" as const };
		expect(shownFields({ fields: picked, from: current }, moved)).toBe(moved);
	});
});

describe("draftOf", () => {
	it("keeps what you typed only while the value is still the one you started from", () => {
		expect(draftOf({ text: "12", from: 5 }, 5)).toBe("12");
		expect(draftOf({ text: "12", from: 5 }, 7)).toBeNull();
		expect(draftOf(null, 5)).toBeNull();
	});
});

describe("someCards", () => {
	it("gives a Mac its sleep and Cursor cards and its app keys", () => {
		expect(someCards(view, "dev-mac")).toEqual({
			mac: true,
			workspaces: false,
			library: false,
			network: false,
			appKeys: true,
		});
	});

	it("gives an Incus host its workspaces, and what its maxd offers", () => {
		expect(someCards(view, "dev-box")).toEqual({
			mac: false,
			workspaces: true,
			library: true,
			network: true,
			appKeys: false,
		});
	});
});
