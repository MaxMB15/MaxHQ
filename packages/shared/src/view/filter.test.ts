// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { searchWords } from "../lib/words";
import {
	agent,
	device,
	entities,
	link,
	MIN,
	network,
	NOW,
	project,
	workspace,
} from "../test/build";

import { filterSidebar, type SidebarLists } from "./filter";
import { DEFAULT_INBOX } from "./inbox";
import { applySnapshot } from "./replica";
import { hiddenProjectList, inboxSections, projectGroups } from "./select";

const DAY = 24 * 60 * MIN;

const view = applySnapshot({
	seq: 1,
	entities: entities({
		projects: [project("p-mosaic", "Mosaïc"), project("p-comet", "Comet")],
		devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-box" })],
		networks: [network()],
		links: [
			link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
			link({ id: "box", name: "devbox", device: "dev-box", network: "tailnet" }),
		],
		workspaces: [
			workspace({ id: "w-mosaic", project: "p-mosaic", device: "dev-mac" }),
			workspace({ id: "w-comet", project: "p-comet", device: "dev-box" }),
			workspace({ id: "w-loose", project: null, device: "dev-mac" }),
		],
		agents: [
			agent({ id: "tiles", title: "Fix the tile cache", workspace: "w-mosaic" }),
			agent({
				id: "export",
				title: "Café export",
				branch: "feat/pdf-export",
				workspace: "w-mosaic",
			}),
			agent({
				id: "folded",
				title: "Old tile notes",
				workspace: "w-mosaic",
				last_activity_at: NOW - 3 * DAY,
			}),
			agent({ id: "sync", title: "Subtitle sync drift", workspace: "w-comet" }),
			agent({
				id: "pinned",
				title: "Release checklist",
				workspace: "w-comet",
				inbox: { ...DEFAULT_INBOX, pin: { at: NOW, key: "a0" } },
			}),
			agent({ id: "loose", title: "Scratch", workspace: "w-loose" }),
		],
	}),
});

const lists: SidebarLists = {
	groups: projectGroups(view, NOW),
	sections: inboxSections(view, NOW),
	hidden: [project("p-atlas", "Atlas"), project("p-tiles", "Tile lab")],
};

const filter = (query: string) => filterSidebar(view, NOW, lists, searchWords(query));
const rows = (shown: SidebarLists) =>
	Object.fromEntries(shown.groups.map((g) => [g.name, g.agents.map((a) => a.id)]));

describe("the sidebar filter", () => {
	it("hands the lists back as they are with no words", () => {
		expect(filter("")).toBe(lists);
		expect(filter("   ")).toBe(lists);
	});

	it("keeps the agents whose title holds every word, ignoring case and accents", () => {
		expect(rows(filter("CAFE"))).toEqual({ Mosaïc: ["export"] });
		expect(rows(filter("tile fix"))).toEqual({ Mosaïc: ["tiles"] });
		expect(rows(filter("tile cafe"))).toEqual({});
	});

	it("shows a folded agent that matches with the rest", () => {
		const shown = filter("tile");
		expect(rows(shown)).toEqual({ Mosaïc: ["tiles", "folded"] });
		expect(shown.groups[0]!.older).toEqual([]);
	});

	it("finds an agent by its branch", () => {
		expect(rows(filter("pdf"))).toEqual({ Mosaïc: ["export"] });
	});

	it("keeps every agent of a project whose name matches", () => {
		expect(rows(filter("mosaic"))).toEqual({ Mosaïc: ["export", "tiles", "folded"] });
		expect(filter("comet").sections.pinned.map((a) => a.id)).toEqual(["pinned"]);
	});

	it("finds an agent by its device, and words can sit in different fields", () => {
		const shown = filter("devbox");
		expect(rows(shown)).toEqual({ Comet: ["sync"] });
		expect(shown.groups[0]!.devices).toEqual(["devbox"]);
		expect(rows(filter("mac scratch"))).toEqual({ "No project": ["loose"] });
	});

	it("drops the projects left with no agent and filters the other lists too", () => {
		const shown = filter("checklist");
		expect(shown.groups).toEqual([]);
		expect(shown.sections.pinned.map((a) => a.id)).toEqual(["pinned"]);
		expect(filter("subtitle").sections.pinned).toEqual([]);
	});

	it("keeps the hidden projects whose names hold the words", () => {
		expect(filter("tile").hidden.map((p) => p.name)).toEqual(["Tile lab"]);
		expect(filter("devbox").hidden).toEqual([]);
	});

	it("keeps a caller when an agent it started matches", () => {
		const started = applySnapshot({
			seq: 1,
			entities: entities({
				devices: [device({ id: "dev-mac", os: "macos" })],
				links: [
					link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
				],
				workspaces: [workspace({ id: "w1", project: "p", device: "dev-mac" })],
				projects: [project("p", "MaxHQ")],
				agents: [
					agent({ id: "caller", title: "Release checklist" }),
					agent({
						id: "child",
						title: "Bump the version",
						origin: {
							permission: "perm-1",
							by: { kind: "agent", agent: "caller" },
							device: "dev-mac",
						},
					}),
					agent({ id: "other", title: "Write the notes" }),
				],
			}),
		});
		const all: SidebarLists = {
			groups: projectGroups(started, NOW),
			sections: inboxSections(started, NOW),
			hidden: hiddenProjectList(started),
		};
		const shown = filterSidebar(started, NOW, all, searchWords("bump"));
		expect(rows(shown)).toEqual({ MaxHQ: ["caller"] });
	});
});
