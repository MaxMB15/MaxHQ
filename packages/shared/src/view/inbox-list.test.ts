// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Activity, Inbox } from "../bindings";
import { agent, attention, entities, link, MIN, NOW, project, workspace } from "../test/build";

import { DEFAULT_INBOX } from "./inbox";
import { inboxList, swipeMenu } from "./inbox-list";
import { applySnapshot } from "./replica";

const inbox = (over: Partial<Inbox>): Inbox => ({ ...DEFAULT_INBOX, ...over });

const view = applySnapshot({
	seq: 1,
	entities: entities({
		links: [
			link({
				id: "devbox",
				device: "dev-box",
				capabilities: { transfers: true, tunnels: true, workspaces: true, inbox: true },
			}),
			link({ id: "mac", device: "dev-mac", route: { type: "local" } }),
		],
		projects: [project("p1", "notes-app")],
		workspaces: [
			workspace({ id: "w1", device: "dev-box" }),
			workspace({ id: "w2", project: "p1" }),
		],
		agents: [
			agent({ id: "idle", last_activity_at: NOW - 30 * MIN }),
			agent({ id: "done", activity: { state: "done_unread" } }),
			agent({ id: "busy", activity: { state: "busy" }, started_at: NOW - 5 * MIN }),
			agent({ id: "busy-old", activity: { state: "busy" }, started_at: NOW - 50 * MIN }),
			agent({ id: "ask-new", activity: { state: "needs_input", attention: "e-new" } }),
			agent({
				id: "ask-old",
				activity: { state: "needs_input", attention: "e-old" },
				workspace: "w2",
			}),
			agent({ id: "pinned", inbox: inbox({ pin: { at: NOW, key: "m" } }) }),
			agent({ id: "snoozed", inbox: inbox({ snoozed_until: NOW + 60 * MIN }) }),
			agent({ id: "settled", inbox: inbox({ settled: { at: NOW - MIN, by: "you" } }) }),
			agent({ id: "sub", parent: "busy", activity: { state: "busy" } }),
			agent({
				id: "started",
				workspace: "w2",
				origin: {
					permission: "perm-1",
					by: { kind: "agent", agent: "ask-old" },
					device: "dev-mac",
				},
			}),
			agent({ id: "old", last_activity_at: NOW - 2 * 24 * 60 * MIN }),
		],
		attention: [
			attention({ id: "e-new", agent: "ask-new", created_at: NOW - MIN }),
			attention({ id: "e-old", agent: "ask-old", created_at: NOW - 9 * MIN }),
		],
	}),
});

describe("inboxList", () => {
	const list = inboxList(view, NOW);
	const ids = (rows: readonly { agent: { id: string } }[]) => rows.map((r) => r.agent.id);

	it("puts the longest waiting first", () => {
		expect(list.waiting.map((w) => w.agent.id)).toEqual(["ask-old", "ask-new"]);
	});

	it("groups the rest by project, by rank, with the ones without one last", () => {
		expect(list.groups.map((g) => g.name)).toEqual(["notes-app", "No project"]);
		expect(ids(list.groups[1]!.rows)).toEqual(["ask-new", "busy", "busy-old", "done", "idle"]);
		expect(ids(list.groups[1]!.older)).toEqual(["old"]);
	});

	it("keeps a waiting agent's row in its group, as the desktop does", () => {
		expect(ids(list.groups[0]!.rows)).toContain("ask-old");
	});

	it("puts an agent another one started under it, a level in", () => {
		expect(list.groups[0]!.rows).toEqual([
			{ agent: view.tables.agent.get("ask-old"), level: 0 },
			{ agent: view.tables.agent.get("started"), level: 1 },
		]);
	});

	it("keeps pinned, snoozed and settled agents out of the groups", () => {
		expect(ids(list.pinned)).toEqual(["pinned"]);
		expect(list.snoozed.map((a) => a.id)).toEqual(["snoozed"]);
		expect(list.settled.map((a) => a.id)).toEqual(["settled"]);
		const grouped = list.groups.flatMap((g) => ids([...g.rows, ...g.older]));
		expect(grouped).not.toContain("pinned");
		expect(grouped).not.toContain("sub");
	});

	it("leaves out a hidden project while its agents are quiet, as the desktop does", () => {
		const groups = (activity: Activity) =>
			inboxList(
				applySnapshot({
					seq: 1,
					entities: [
						...entities({
							projects: [project("p1", "notes-app")],
							workspaces: [workspace({ id: "w2", project: "p1" })],
							agents: [agent({ id: "a", workspace: "w2", activity })],
						}),
						{ entity: "hidden_projects", device: "dev-mac", projects: ["p1"] },
					],
				}),
				NOW
			).groups.map((g) => g.name);
		expect(groups({ state: "idle" })).toEqual([]);
		expect(groups({ state: "busy" })).toEqual(["notes-app"]);
	});

	it("lists hidden projects, and marks a hidden one that shows", () => {
		const hidden = inboxList(
			applySnapshot({
				seq: 1,
				entities: [
					...entities({
						projects: [project("p1", "notes-app"), project("p2", "api")],
						workspaces: [
							workspace({ id: "w1", project: "p1" }),
							workspace({ id: "w2", project: "p2" }),
						],
						agents: [agent({ id: "a", workspace: "w1", activity: { state: "busy" } })],
					}),
					{ entity: "hidden_projects", device: "dev-mac", projects: ["p1", "p2"] },
				],
			}),
			NOW
		);
		expect(hidden.hidden.map((p) => p.name)).toEqual(["api", "notes-app"]);
		expect(hidden.groups.map((g) => [g.name, g.hidden])).toEqual([["notes-app", true]]);
		expect(list.groups.every((g) => !g.hidden)).toBe(true);
		expect(list.hidden).toEqual([]);
	});

	it("moves busy agents to the Working shelf when it's on", () => {
		expect(list.working).toEqual([]);
		const shelved = inboxList(view, NOW, { workingShelf: true });
		expect(ids(shelved.working)).toEqual(["busy", "busy-old"]);
		expect(ids(shelved.groups[1]!.rows)).toEqual(["ask-new", "done", "idle"]);
	});

	it("keeps the agents the filter's words find, and everyone waiting on you", () => {
		const found = inboxList(view, NOW, { words: ["notes"] });
		expect(found.groups.map((g) => g.name)).toEqual(["notes-app"]);
		expect(ids(found.pinned)).toEqual([]);
		expect(found.waiting.map((w) => w.agent.id)).toEqual(["ask-old", "ask-new"]);
		const older = inboxList(view, NOW, { words: ["old"] });
		expect(ids(older.groups.flatMap((g) => g.rows))).toEqual([
			"ask-old",
			"started",
			"busy-old",
			"old",
		]);
		expect(older.groups.flatMap((g) => g.older)).toEqual([]);
	});
});

describe("swipeMenu", () => {
	const of = (id: string) => view.tables.agent.get(id)!;

	it("offers settle and snooze where the device keeps inboxes", () => {
		const menu = swipeMenu(view, of("idle"), NOW);
		expect(menu?.settle?.label).toBe("Settle");
		expect(menu?.snooze).toEqual(["idle"]);
		expect(swipeMenu(view, of("settled"), NOW)?.settle?.label).toBe("Unsettle");
	});

	it("offers nothing where it doesn't, or for a sub-agent", () => {
		expect(swipeMenu(view, of("ask-old"), NOW)).toBeNull();
		expect(swipeMenu(view, of("sub"), NOW)).toBeNull();
	});
});
