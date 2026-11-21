// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	DEVICE_CARDS,
	deviceCardPlace,
	OLD_DEVICE_CARDS,
	OLD_SETTINGS_PAGES,
	PAGE_CARDS,
	PHONE_CARDS,
	SETTINGS_GROUPS,
	settingsPlace,
} from "./settings-link";

describe("an old Settings link", () => {
	it("opens where each page from before lives now", () => {
		expect(
			Object.fromEntries(
				Object.keys(OLD_SETTINGS_PAGES).map((name) => [name, settingsPlace(name)])
			)
		).toEqual({
			appearance: { kind: "page", at: "appearance" },
			sidebar: { kind: "page", at: "sidebar" },
			editor: { kind: "page", at: "open-in" },
			dispatcher: { kind: "page", at: "dispatcher" },
			"agent-tools": { kind: "page", at: "permissions" },
			"local-models": { kind: "local-models" },
			"scheduled-tasks": { kind: "scheduled-tasks" },
			"test-hosts": { kind: "device", device: null, card: "test-machines" },
			skills: { kind: "skills" },
			notifications: { kind: "page", at: "notifications" },
			updates: { kind: "page", at: "updates" },
			phones: { kind: "phones", card: "phones" },
			shortcuts: { kind: "shortcuts" },
			claude: { kind: "login", login: "claude" },
			codex: { kind: "login", login: "codex" },
			cursor: { kind: "login", login: "cursor" },
			github: { kind: "login", login: "github" },
			apns: { kind: "phones", card: "apns" },
		});
	});

	it("opens a device's tab at the card each of its old cards went to", () => {
		expect(
			Object.fromEntries(
				Object.keys(OLD_DEVICE_CARDS).map((card) => [card, deviceCardPlace("devbox", card)])
			)
		).toEqual({
			"new-agents": { kind: "device", device: "devbox", card: "new-agents" },
			limits: { kind: "device", device: "devbox", card: "new-agents" },
			placement: { kind: "device", device: "devbox", card: "new-agents" },
			inbox: { kind: "device", device: "devbox", card: "inbox" },
			idle: { kind: "device", device: "devbox", card: "resources" },
			memory: { kind: "device", device: "devbox", card: "resources" },
			sleep: { kind: "device", device: "devbox", card: "resources" },
			terminals: { kind: "device", device: "devbox", card: "terminals" },
			"setup-tokens": { kind: "device", device: "devbox", card: "setup-tokens" },
			workspaces: { kind: "device", device: "devbox", card: "workspaces" },
			library: { kind: "device", device: "devbox", card: "library" },
			network: { kind: "device", device: "devbox", card: "network" },
			"test-hosts": { kind: "device", device: "devbox", card: "test-machines" },
			"other-agents": { kind: "page", at: "other-agents" },
			cursor: { kind: "page", at: "cursor-usage" },
			"app-keys": { kind: "page", at: "app-keys" },
		});
	});
});

describe("a Settings link", () => {
	it("opens each group and card by its own name", () => {
		for (const name of [...SETTINGS_GROUPS, ...PAGE_CARDS])
			expect(settingsPlace(name)).toEqual({ kind: "page", at: name });
		for (const card of DEVICE_CARDS)
			expect(settingsPlace(card)).toEqual({ kind: "device", device: null, card });
		for (const card of PHONE_CARDS)
			expect(settingsPlace(card)).toEqual({ kind: "phones", card });
		for (const card of DEVICE_CARDS)
			expect(deviceCardPlace("devbox", card)).toEqual({
				kind: "device",
				device: "devbox",
				card,
			});
	});

	it("opens at the top for no name or one Settings doesn't know", () => {
		expect(settingsPlace(null)).toBeNull();
		expect(settingsPlace("constructor")).toBeNull();
		expect(settingsPlace("nothing")).toBeNull();
		expect(deviceCardPlace("devbox", "nothing")).toEqual({
			kind: "device",
			device: "devbox",
			card: null,
		});
		expect(deviceCardPlace("devbox", "toString")).toEqual({
			kind: "device",
			device: "devbox",
			card: null,
		});
	});
});
