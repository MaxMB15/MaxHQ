// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { searchWords } from "@maxhq/shared/lib/words";

import { type Results, type SearchIndex, searchSettings, searchTab } from "./find";

const INDEX: SearchIndex = {
	groups: [
		{ group: "general", label: "General" },
		{ group: "chat", label: "Chat and agents" },
		{ group: "accounts", label: "Accounts" },
		{ group: "devices", label: "Devices and phones" },
		{ group: "permissions", label: "Permissions" },
		{ group: "projects", label: "Projects" },
	],
	cards: [
		{ card: "appearance", group: "general", title: "Appearance", terms: ["Theme"] },
		{ card: "composer", group: "chat", title: "Composer", terms: ["Check spelling"] },
		{ card: "accounts", group: "accounts", title: "Logins", terms: ["Signed in"] },
		{ card: "grants", group: "permissions", title: "Grants", terms: ["Revoke"] },
		{ card: "app-keys", group: "permissions", title: "App keys", terms: [] },
		{ card: "projects", group: "projects", title: "Projects", terms: [] },
	],
	details: [
		{
			key: "claude",
			group: "accounts",
			place: { kind: "login", login: "claude" },
			title: "Claude",
			terms: ["Setup token"],
		},
		{
			key: "p-app",
			group: "projects",
			place: { kind: "project", project: "p-app" },
			title: "example-app",
			terms: ["Resume after a restart"],
		},
		{
			key: "p-app",
			group: "projects",
			place: { kind: "project", project: "p-app", tab: "skills" },
			title: "example-app",
			terms: ["Skills"],
		},
		{
			key: "local-models",
			group: "views",
			place: { kind: "local-models" },
			title: "Local models",
			terms: ["Ollama"],
		},
	],
	devices: [
		{
			device: "dev-mac",
			name: "Home Mac",
			cards: [
				{ id: "connection", terms: ["Connection", "Device ID"] },
				{ id: "inbox", terms: ["Inbox", "Snooze at the limit"] },
			],
		},
		{
			device: "dev-devbox",
			name: "devbox",
			cards: [
				{ id: "connection", terms: ["Connection", "Device ID"] },
				{ id: "inbox", terms: ["Inbox", "Snooze at the limit"] },
				{ id: "resources", terms: ["Resources", "Memory"] },
			],
		},
	],
	phones: [
		{ card: "phones", title: "Phones", terms: ["Add phone"] },
		{ card: "apns", title: "Phone pushes", terms: ["Push key"] },
	],
};

const search = (query: string): Results => searchSettings(INDEX, searchWords(query))!;

describe("searching settings", () => {
	it("shows the whole page with no words", () => {
		expect(searchSettings(INDEX, [])).toBeNull();
	});

	it("keeps the cards that have the words, and their groups", () => {
		const results = search("spelling");
		expect([...results.cards]).toEqual(["composer"]);
		expect([...results.groups]).toEqual(["chat"]);
		expect(results.hits.map((hit) => hit.key)).toEqual(["composer"]);
		expect(results.hits[0]!.found.term).toEqual([
			{ text: "Check ", hit: false },
			{ text: "spelling", hit: true },
		]);
	});

	it("keeps a group's every card when its name matches", () => {
		expect([...search("permissions").cards]).toEqual(["grants", "app-keys"]);
	});

	it("finds a view of its own by its terms, and keeps the card that leads to it", () => {
		const results = search("setup token");
		expect(results.hits).toMatchObject([
			{ kind: "detail", place: { kind: "login", login: "claude" } },
		]);
		expect([...results.groups]).toEqual(["accounts"]);
		expect(results.cards.has("accounts")).toBe(true);
	});

	it("opens a project on the tab that has the words, and lists only it", () => {
		const results = search("skills");
		expect(results.hits).toMatchObject([
			{ kind: "detail", place: { kind: "project", project: "p-app", tab: "skills" } },
		]);
		expect(results.projects).toEqual(new Set(["p-app"]));
		expect(search("projects").projects).toBeNull();
	});

	it("puts views of their own last", () => {
		expect(search("ollama").hits).toMatchObject([{ kind: "detail", group: "views" }]);
		expect([...search("ollama").groups]).toEqual(["views"]);
	});
});

describe("a device's hit", () => {
	it("opens at the device's first card with the words, and keeps each such card", () => {
		const results = search("snooze");
		expect(results.hits).toMatchObject([
			{ kind: "device", device: "dev-mac", card: "inbox" },
			{ kind: "device", device: "dev-devbox", card: "inbox" },
		]);
		expect(results.devices.get("dev-devbox")).toEqual(new Set(["inbox"]));
	});

	it("opens at the tab's top and keeps the whole tab when the name matches", () => {
		const results = search("devbox");
		expect(results.hits).toMatchObject([{ kind: "device", device: "dev-devbox", card: null }]);
		expect(results.devices.get("dev-devbox")).toBeNull();
		expect(results.devices.has("dev-mac")).toBe(false);
	});

	it("keeps every tab whole when the group's name matches", () => {
		const results = search("devices");
		expect(results.devices).toEqual(
			new Map([
				["dev-mac", null],
				["dev-devbox", null],
			])
		);
		expect([...results.phones]).toEqual(["phones", "apns"]);
	});

	it("matches the name and a card's terms together", () => {
		const results = search("devbox memory");
		expect(results.hits).toMatchObject([{ device: "dev-devbox", card: "resources" }]);
	});

	it("shows the tab you picked when it has a hit, else the first device with one", () => {
		const results = search("memory");
		expect(searchTab("dev-devbox", results)).toBe("dev-devbox");
		expect(searchTab("dev-mac", results)).toBe("dev-devbox");
		expect(searchTab("phones", results)).toBe("dev-devbox");
	});

	it("shows Phones when only a phone card matched, and no tab when nothing did", () => {
		expect(searchTab("dev-mac", search("push key"))).toBe("phones");
		expect([...search("push key").phones]).toEqual(["apns"]);
		expect(searchTab("dev-mac", search("spelling"))).toBeNull();
	});
});
