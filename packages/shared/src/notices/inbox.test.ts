// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { Agent, Notice } from "../bindings";
import { mockNotices } from "../data/mock-notices";
import { agent, attention, device, entities, MIN, NOW, project, workspace } from "../test/build";
import { BADGE_REASONS, badgeCount, DEFAULT_BADGE_PREFS } from "../view/badge";
import { applySnapshot, type ViewState } from "../view/replica";

import {
	bellCount,
	createNoticeLog,
	earlierRows,
	finishedAgents,
	hasMore,
	NOTICE_PAGE,
	nowRows,
} from "./inbox";
import type { NoticeCalls } from "./requests";

const viewOf = (agents: Agent[], hidden: string[] = []): ViewState =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [device({ id: "dev-mac", name: "Home Mac" })],
				projects: [project("p1", "example-app"), project("p2", "hidden-app")],
				workspaces: [
					workspace({ id: "w1", project: "p1" }),
					workspace({ id: "w2", project: "p2" }),
					workspace({ id: "w3" }),
				],
				attention: [
					attention({ id: "e-ask", agent: "ask", created_at: NOW - 5 * MIN }),
					attention({ id: "e-q", agent: "question", kind: "question" }),
				],
				agents,
			}),
			{ entity: "hidden_projects" as const, device: "dev-mac", projects: hidden },
		],
	});

const ask = agent({
	id: "ask",
	title: "Release checklist",
	activity: { state: "needs_input", attention: "e-ask" },
});
const question = agent({
	id: "question",
	workspace: "w3",
	activity: { state: "needs_input", attention: "e-q" },
	last_activity_at: NOW,
});
const done = agent({ id: "done", activity: { state: "done_unread" }, last_activity_at: NOW });
const crashed = agent({
	id: "crashed",
	activity: { state: "error", reason: "exit 1" },
	last_activity_at: NOW - 10 * MIN,
});
const hidden = agent({ id: "hidden", workspace: "w2", activity: { state: "done_unread" } });
const sub = agent({ id: "sub", parent: "ask", activity: { state: "done_unread" } });
const idle = agent({ id: "idle" });

describe("the Now list", () => {
	const view = viewOf([ask, question, done, crashed, hidden, sub, idle], ["p2"]);

	it("holds exactly what the badge counts, for every choice of reasons", () => {
		const choices = [[], ...BADGE_REASONS.map((r) => [r]), BADGE_REASONS];
		for (const counts of choices) {
			expect(nowRows(view, NOW, counts)).toHaveLength(
				badgeCount(view, NOW, { show: true, counts })
			);
		}
		// Newest first: a prompt counts from when it asked, the rest from
		// their last activity.
		expect(nowRows(view, NOW, DEFAULT_BADGE_PREFS.counts).map((row) => row.agent)).toEqual([
			"done",
			"question",
			"ask",
			"crashed",
		]);
	});

	it("says what each agent wants, where it runs and whether it stays", () => {
		const rows = nowRows(view, NOW, BADGE_REASONS);
		expect(rows.find((row) => row.agent === "ask")).toMatchObject({
			title: "Release checklist",
			place: "example-app",
			what: "Asking for permission",
			at: NOW - 5 * MIN,
			stays: "Stays until you answer it",
		});
		expect(rows.find((row) => row.agent === "question")).toMatchObject({
			place: "Home Mac",
			what: "Asking you a question",
		});
		expect(rows.find((row) => row.agent === "done")).toMatchObject({
			what: "Done, not opened yet",
			stays: undefined,
		});
		expect(rows.find((row) => row.agent === "crashed")).toMatchObject({
			what: "Stopped with an error",
			stays: "Stays until it runs again",
		});
		expect(finishedAgents(view, NOW, BADGE_REASONS)).toEqual(["done"]);
	});

	it("gives the bell a count only while Show the badge is on", () => {
		const rows = nowRows(view, NOW, BADGE_REASONS);
		expect(bellCount(rows, DEFAULT_BADGE_PREFS)).toBe(4);
		expect(bellCount([], DEFAULT_BADGE_PREFS)).toBe(0);
		// Off, the bell can't say a number, nor that nothing needs you.
		expect(bellCount(rows, { ...DEFAULT_BADGE_PREFS, show: false })).toBeNull();
		expect(bellCount([], { ...DEFAULT_BADGE_PREFS, show: false })).toBeNull();
	});
});

const notice = (over: Partial<Notice> & { id: number }): Notice => ({
	device: "dev-mac",
	agent: "ask",
	kind: "finished",
	title: "Release checklist",
	project: "example-app",
	summary: null,
	at: NOW - over.id * MIN,
	read_at: null,
	...over,
});

describe("the Earlier list", () => {
	it("shows each notice's words and marks the ones whose agent is gone", () => {
		const view = viewOf([ask]);
		const rows = earlierRows(view, [
			notice({ id: 1, kind: "limit" }),
			notice({ id: 2, kind: "question", summary: "Which branch?", read_at: NOW }),
			notice({ id: 3, agent: "gone", project: null }),
		]);
		expect(
			rows.map(({ what, unread, gone, place }) => ({ what, unread, gone, place }))
		).toEqual([
			{ what: "Hit a usage limit", unread: true, gone: false, place: "example-app" },
			{ what: "Which branch?", unread: false, gone: false, place: "example-app" },
			{ what: "Finished its turn", unread: true, gone: true, place: "Home Mac" },
		]);
	});
});

describe("the notice log", () => {
	const logOver = (
		calls: NoticeCalls,
		markSeen = vi.fn(async () => ({ status: "ok" as const, data: null }))
	) => ({ log: createNoticeLog({ ...calls, markSeen }, () => NOW), markSeen });

	it("loads a page and brings more a page at a time", async () => {
		const { log } = logOver(mockNotices(() => NOW));
		await log.load();
		const first = log.store.getState().page;
		expect(first?.notices).toHaveLength(NOTICE_PAGE);
		expect(first && hasMore(first)).toBe(true);
		await log.more();
		const all = log.store.getState().page;
		expect(all?.notices).toHaveLength(27);
		expect(all && hasMore(all)).toBe(false);
		const times = all?.notices.map((n) => n.at) ?? [];
		expect(times).toEqual([...times].sort((a, b) => b - a));
	});

	it("marks every notice read and settles the finished agents", async () => {
		const calls = mockNotices(() => NOW);
		const { log, markSeen } = logOver(calls);
		await log.load();
		expect(log.store.getState().page?.unread).toBe(4);
		await log.readAll(["done"]);
		expect(markSeen).toHaveBeenCalledWith("done");
		expect(log.store.getState().page?.unread).toBe(0);
		await log.load();
		expect(log.store.getState().page?.notices.every((n) => n.read_at !== null)).toBe(true);
	});

	it("marks one agent's notices read on its own", async () => {
		const calls = mockNotices(() => NOW);
		await calls.readNotices("agent-release");
		const page = await calls.listNotices(500);
		const unread = page.status === "ok" ? page.data.notices.filter((n) => !n.read_at) : [];
		expect(unread.map((n) => n.agent)).not.toContain("agent-release");
		expect(unread.length).toBeGreaterThan(0);
	});

	it("clears every notice", async () => {
		const { log } = logOver(mockNotices(() => NOW));
		await log.load();
		await log.clear();
		expect(log.store.getState().page).toEqual({ notices: [], total: 0, unread: 0 });
		await log.load();
		expect(log.store.getState().page?.total).toBe(0);
	});

	it("loads again and says why when a call fails", async () => {
		const calls = mockNotices(() => NOW);
		const { log } = logOver({
			...calls,
			clearNotices: async () => ({
				status: "error",
				error: { kind: "not_connected", device: "devbox" },
			}),
		});
		await log.load();
		await log.clear();
		expect(log.store.getState().failed).toBe("devbox isn't connected");
		expect(log.store.getState().page?.total).toBe(27);
	});
});
