// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Inbox } from "../bindings";
import { agent, device, entities, link, MIN, NOW, project, workspace } from "../test/build";
import { DEFAULT_INBOX } from "../view/inbox";
import { applySnapshot } from "../view/replica";
import { projectGroups } from "../view/select";

import { draggedAgents, draggedName, foldKey, shelfRows, shownRows } from "./rows";
import { createUiStore, SETTLED_SHOWN } from "./store";

const DAY = 24 * 60 * MIN;
const inbox = (over: Partial<Inbox>): Inbox => ({ ...DEFAULT_INBOX, ...over });
const settled = Array.from({ length: SETTLED_SHOWN + 2 }, (_, i) =>
	agent({
		id: `settled-${i}`,
		workspace: "w-a",
		inbox: inbox({ settled: { at: NOW - (i + 1) * MIN, by: "you" } }),
	})
);

const view = applySnapshot({
	seq: 1,
	entities: entities({
		projects: [project("p-a", "Alpha"), project("p-b", "Beta")],
		devices: [device({ id: "dev-mac", os: "macos" })],
		links: [link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } })],
		workspaces: [
			workspace({ id: "w-a", project: "p-a", device: "dev-mac" }),
			workspace({ id: "w-b", project: "p-b", device: "dev-mac" }),
		],
		agents: [
			agent({ id: "pin", workspace: "w-b", inbox: inbox({ pin: { at: NOW, key: "a0" } }) }),
			agent({ id: "a1", workspace: "w-a", last_activity_at: NOW - MIN }),
			agent({ id: "a-old", workspace: "w-a", last_activity_at: NOW - 3 * DAY }),
			agent({ id: "b1", workspace: "w-b", last_activity_at: NOW - MIN }),
			agent({ id: "busy", workspace: "w-b", activity: { state: "busy" } }),
			agent({ id: "snoozed", workspace: "w-a", inbox: inbox({ snoozed_until: NOW + DAY }) }),
			...settled,
		],
	}),
});

const options = { workingShelf: true };
const [alpha] = projectGroups(view, NOW, options);

describe("the rows the projects view shows", () => {
	it("lists the pins and the groups, but not a closed shelf or a folded agent", () => {
		expect(shownRows(view, NOW, options, createUiStore().getState())).toEqual([
			"pin",
			"a1",
			"b1",
		]);
	});

	it("leaves out a collapsed group and adds the older agents you show", () => {
		const ui = createUiStore({ collapsed: new Set([alpha!.key]) }).getState();
		expect(shownRows(view, NOW, options, ui)).toEqual(["pin", "b1"]);
		const older = createUiStore({ showingOlder: new Set([alpha!.key]) }).getState();
		expect(shownRows(view, NOW, options, older)).toEqual(["pin", "a1", "a-old", "b1"]);
	});

	it("follows the filter: its matches, with every group and shelf open unless folded since", () => {
		const ui = createUiStore({ collapsed: new Set([alpha!.key]) }).getState();
		const ids = shownRows(view, NOW, options, ui, { words: ["alpha"], folded: new Set() });
		expect(ids.slice(0, 3)).toEqual(["a1", "a-old", "snoozed"]);
		expect(ids).toHaveLength(3 + SETTLED_SHOWN);
		const folded = new Set([foldKey("project", alpha!.key), foldKey("shelf", "settled")]);
		expect(shownRows(view, NOW, options, ui, { words: ["alpha"], folded })).toEqual([
			"snoozed",
		]);
	});

	it("puts an agent another one started right under it", () => {
		const started = agent({
			id: "started",
			workspace: "w-b",
			origin: { permission: "perm-1", by: { kind: "agent", agent: "a1" }, device: "dev-mac" },
		});
		const withStarted = applySnapshot({
			seq: 2,
			entities: [...view.tables.agent.values(), started].map((a) => ({
				entity: "agent" as const,
				...a,
			})),
		});
		const ids = shownRows(withStarted, NOW, options, createUiStore().getState());
		expect(ids).toContain("started");
		expect(ids.indexOf("started")).toBe(ids.indexOf("a1") + 1);
	});

	it("adds each open shelf at the bottom, a page of the settled ones", () => {
		const ui = createUiStore({
			openShelves: new Set(["working", "snoozed", "settled"] as const),
		}).getState();
		expect(shownRows(view, NOW, options, ui)).toEqual([
			"pin",
			"a1",
			"b1",
			"busy",
			"snoozed",
			...settled.slice(0, SETTLED_SHOWN).map((a) => a.id),
		]);
		expect(shownRows(view, NOW, { workingShelf: false }, ui)).toContain("busy");
	});

	it("keeps the agent you're looking at in a closed shelf, or past the settled page", () => {
		const looking = createUiStore({ selectedAgent: "snoozed" }).getState();
		expect(shelfRows("snoozed", [view.tables.agent.get("snoozed")!], looking)).toHaveLength(1);
		expect(shelfRows("settled", settled, looking)).toEqual([]);

		const last = settled.at(-1)!;
		const open = createUiStore({
			openShelves: new Set(["settled"] as const),
			selectedAgent: last.id,
		}).getState();
		expect(shelfRows("settled", settled, open).map((a) => a.id)).toEqual([
			...settled.slice(0, SETTLED_SHOWN).map((a) => a.id),
			last.id,
		]);
	});
});

describe("the agents a drag carries", () => {
	const a1 = view.tables.agent.get("a1")!;

	it("carries the picked agents when the row is picked, the pressed one first", () => {
		const dragged = draggedAgents(view, a1, new Set(["b1", "a1", "gone"]));
		expect(dragged.map((a) => a.id)).toEqual(["a1", "b1"]);
		expect(draggedName(dragged)).toBe("2 agents");
	});

	it("carries only the row when it isn't picked", () => {
		const dragged = draggedAgents(view, a1, new Set(["b1"]));
		expect(dragged.map((a) => a.id)).toEqual(["a1"]);
		expect(draggedName(dragged)).toBe("a1");
	});
});
