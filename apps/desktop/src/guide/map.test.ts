// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { APNS_LOGIN_TERMS } from "@/settings/apns-login";
import { deviceCards } from "@/settings/device-settings";
import { LOGIN_STEPS } from "@/settings/login-steps";
import { LOGIN_TERMS, ownViews, PAGE } from "@/settings/page/cards";
import { PHONES_TERMS } from "@/settings/phones-settings";
import { PROJECT_TERMS } from "@/settings/project-settings";
import { shortcutRows } from "@/shortcut-list/rows";
import { PROJECT_SKILLS_TERMS } from "@/skills/skills-tab";
import { LOGIN_VIEWS } from "@maxhq/shared/guide/settings-link";
import { VIEWS } from "@maxhq/shared/guide/view-link";
import { LOGIN_HINT } from "@maxhq/shared/settings/login-steps";
import { device, entities, link } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import mapText from "../../../../guide/map.json?raw";
import { buildMap, type MapSources, renderMap, type SettingsEntry, staleParts } from "./map";

afterEach(() => {
	vi.restoreAllMocks();
});

/** The map, from this test file. */
const MAP = "../../../../guide/map.json";

// A Mac that hosts workspaces, holds the skill library, hosts test
// machines and takes phones over the network, so its tab shows every card
// a device can have.
const EVERY_CARD = applySnapshot({
	seq: 1,
	entities: [
		...entities({
			devices: [
				device({ id: "mac", os: "macos", workspace_host: { networks: [], pools: [] } }),
			],
			links: [
				link({
					id: "this-mac",
					device: "mac",
					route: { type: "local" },
					capabilities: {
						transfers: true,
						tunnels: true,
						workspaces: true,
						skills: true,
						test_hosts: true,
					},
				}),
			],
		}),
		{
			entity: "network_access",
			device: "mac",
			on: true,
			port: 1,
			hosts: [],
			pin: null,
			problem: null,
		},
	],
});

/**
 * Every card of the Settings page, each login, the Phones tab's two cards,
 * the views of their own, a project's two tabs and every device card.
 */
const appSettings = (): SettingsEntry[] => [
	...PAGE.map((entry) => ({
		id: `settings/${entry.card}`,
		title: entry.title,
		terms: entry.terms,
		link: `maxhq://settings/${entry.card}`,
	})),
	...LOGIN_VIEWS.map((login) => ({
		id: `settings/${login}`,
		title: LOGIN_STEPS[login].title,
		hint: LOGIN_HINT[login],
		terms: LOGIN_TERMS[login],
		link: `maxhq://settings/${login}`,
	})),
	{
		id: "settings/phones",
		title: "Phones",
		terms: PHONES_TERMS,
		link: "maxhq://settings/phones",
	},
	{
		id: "settings/apns",
		title: LOGIN_STEPS.apns.title,
		hint: LOGIN_HINT.apns,
		terms: APNS_LOGIN_TERMS,
		link: "maxhq://settings/apns",
	},
	...ownViews().map((own) => ({
		id: `settings/${own.kind}`,
		title: own.title,
		hint: own.hint,
		terms: own.terms,
		link: `maxhq://settings/${own.kind}`,
	})),
	{
		id: "settings/project",
		title: "A project's settings",
		terms: PROJECT_TERMS,
		link: "maxhq://project/{project}",
	},
	{
		id: "settings/project-skills",
		title: "A project's skills",
		terms: PROJECT_SKILLS_TERMS,
		link: "maxhq://project/{project}/skills",
	},
	...deviceCards(EVERY_CARD, "mac").map(({ id, terms: [title, ...terms] }) => ({
		id: `settings/device/${id}`,
		title: title!,
		terms,
		link: `maxhq://device/{device}/${id}`,
	})),
];

/** What the app has now, with keys as a Mac writes them. */
const appSources = (): MapSources => {
	vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
		"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"
	);
	return { settings: appSettings(), shortcuts: shortcutRows(), views: VIEWS };
};

const FIXTURE: MapSources = {
	settings: [
		{
			id: "settings/editor",
			title: "Editor",
			terms: ["Open in"],
			link: "maxhq://settings/editor",
		},
	],
	shortcuts: [
		{ what: "New agent", group: "Agents", keys: ["⌘N"] },
		{ what: "Next", group: "Inbox", keys: ["⌘]"] },
		{ what: "Next", group: "Find", keys: ["⌘G"] },
	],
	views: [{ id: "view/devices", title: "Devices", link: "maxhq://view/devices" }],
};

describe("the guide's map", () => {
	it("gives each shortcut an id from its words, and its group's when two share them", () => {
		const parts = buildMap(FIXTURE);
		expect(parts.shortcuts.map((entry) => entry.id)).toEqual([
			"shortcut/new-agent",
			"shortcut/inbox-next",
			"shortcut/find-next",
		]);
		expect(parts.shortcuts[0]).toEqual({
			id: "shortcut/new-agent",
			title: "New agent",
			group: "Agents",
			keys: ["⌘N"],
			link: "maxhq://settings/shortcuts",
		});
		expect(parts.settings).toBe(FIXTURE.settings);
	});

	it("keeps the tools part maxd writes, and sorts every key", () => {
		const tools = [{ id: "tool/list_agents", name: "list_agents" }];
		const text = renderMap(buildMap(FIXTURE), { tools, views: [] });
		const map = JSON.parse(text) as Record<string, unknown>;
		expect(Object.keys(map)).toEqual(["settings", "shortcuts", "tools", "views"]);
		expect(map.tools).toEqual(tools);
		expect(text).toContain('{\n      "id": "settings/editor",\n      "link"');
		expect(text.endsWith("}\n")).toBe(true);
	});

	it("names each part that went stale", () => {
		const onDisk = JSON.parse(renderMap(buildMap(FIXTURE), {})) as Record<string, unknown>;
		expect(staleParts(buildMap(FIXTURE), onDisk)).toEqual([]);
		const added: MapSources = {
			...FIXTURE,
			settings: [
				...FIXTURE.settings,
				{
					id: "settings/sidebar",
					title: "Sidebar",
					terms: [],
					link: "maxhq://settings/sidebar",
				},
			],
		};
		expect(staleParts(buildMap(added), onDisk)).toEqual(["settings"]);
	});

	// `pnpm guide:map` runs this with -u, which rewrites the map.
	it("matches the app", async () => {
		const parts = buildMap(appSources());
		const onDisk = JSON.parse(mapText) as Record<string, unknown>;
		const stale = staleParts(parts, onDisk).join(", ");
		await expect(
			renderMap(parts, onDisk),
			`guide/map.json is stale in ${stale}. Run \`pnpm guide:map\`.`
		).toMatchFileSnapshot(MAP);
	});
});
