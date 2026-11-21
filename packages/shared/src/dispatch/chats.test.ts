// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	type Agent,
	type DeviceLink,
	DISPATCHER_CHATS_SINCE,
	type DispatcherChat,
	type NoteSummary,
} from "../bindings";
import type { Entity } from "../fleet/wire";
import type { ActionResult } from "../data/source";
import { agent, entities, link, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	ARCHIVE_AFTER_MS,
	actOnChat,
	chatSections,
	chatsOf,
	chatTitle,
	groupOf,
	isArchived,
	movedAction,
	movedLine,
	movedMarker,
	noteSaveStep,
	searchChats,
	startNoteSave,
	supportsChats,
} from "./chats";
import { createDispatchFlow, startOver } from "./flow";

const chat = (over: Partial<DispatcherChat> & { agent: string }): DispatcherChat => ({
	device: "dev-mac",
	number: 1,
	title: "",
	summary: "",
	project: null,
	active_at: NOW,
	archived: false,
	moved_to: null,
	...over,
});

const macAt = (protocol: number): DeviceLink =>
	link({ id: "mac", device: "dev-mac", route: { type: "local" }, protocol });

const viewOf = (list: Entity[], links: DeviceLink[] = [macAt(DISPATCHER_CHATS_SINCE)]) =>
	applySnapshot({ seq: 1, entities: [...entities({ links }), ...list] });

const asEntity = (c: DispatcherChat): Entity => ({ entity: "dispatcher_chat", ...c });

// A Tuesday afternoon, so the week began yesterday.
const TUESDAY = new Date(2026, 9, 27, 14, 48).getTime();
const at = (day: number, hour: number) => new Date(2026, 9, day, hour, 0).getTime();

describe("the chats list", () => {
	it("needs protocol 53 on the device", () => {
		expect(supportsChats(viewOf([]), "dev-mac")).toBe(true);
		expect(supportsChats(viewOf([], [macAt(DISPATCHER_CHATS_SINCE - 1)]), "dev-mac")).toBe(
			false
		);
		expect(supportsChats(viewOf([], []), "dev-mac")).toBe(false);
	});

	it("lists the device's chats newest first", () => {
		const view = viewOf(
			[
				chat({ agent: "a-old", active_at: NOW - 60_000 }),
				chat({ agent: "a-new" }),
				chat({ agent: "a-other", device: "dev-devbox" }),
			].map(asEntity)
		);
		expect(chatsOf(view, "dev-mac").map((c) => c.agent)).toEqual(["a-new", "a-old"]);
	});

	it("groups by today, this week from Monday, earlier and archived", () => {
		const today = chat({ agent: "today", active_at: at(27, 9) });
		const monday = chat({ agent: "monday", active_at: at(26, 9) });
		const saturday = chat({ agent: "saturday", active_at: at(24, 9) });
		const archived = chat({ agent: "archived", active_at: at(27, 10), archived: true });
		const untouched = chat({ agent: "untouched", active_at: TUESDAY - ARCHIVE_AFTER_MS });
		expect(
			[today, monday, saturday, archived, untouched].map((c) => groupOf(c, TUESDAY))
		).toEqual(["today", "week", "earlier", "archived", "archived"]);
		expect(isArchived(untouched, TUESDAY)).toBe(true);
		expect(isArchived({ ...untouched, active_at: untouched.active_at + 1 }, TUESDAY)).toBe(
			false
		);

		const sections = chatSections([saturday, untouched, today, monday], TUESDAY);
		expect(sections.map((s) => [s.title, s.chats.map((c) => c.agent)])).toEqual([
			["Today", ["today"]],
			["This week", ["monday"]],
			["Earlier", ["saturday"]],
			["Archived", ["untouched"]],
		]);
	});

	it("names an untitled chat by its number", () => {
		expect(chatTitle(chat({ agent: "a", number: 7 }))).toBe("Chat 7");
		expect(chatTitle(chat({ agent: "a", title: "Slow login" }))).toBe("Slow login");
	});

	it("ranks a title match above a summary match, archived chats included", () => {
		const bySummary = chat({ agent: "summary", summary: "The login page is slow" });
		const byTitle = chat({
			agent: "title",
			title: "Login bug",
			active_at: NOW - ARCHIVE_AFTER_MS * 2,
		});
		const neither = chat({ agent: "neither", title: "Release notes" });
		expect(searchChats([bySummary, neither, byTitle], "LOGIN").map((c) => c.agent)).toEqual([
			"title",
			"summary",
		]);
		// Every word has to match somewhere.
		expect(searchChats([bySummary, byTitle], "login slow").map((c) => c.agent)).toEqual([
			"summary",
		]);

		const sections = chatSections([bySummary, neither, byTitle], NOW, " login ");
		expect(sections.map((s) => [s.title, s.chats.map((c) => c.agent)])).toEqual([
			["Matches", ["title", "summary"]],
		]);
		expect(chatSections([neither], NOW, "login")).toEqual([]);
	});
});

describe("the moved marker", () => {
	const source = chat({ agent: "source", title: "Slow login", moved_to: "target" });
	const target = chat({ agent: "target", title: "Release notes" });

	it("shows where the message went in the chat it left", () => {
		const marker = movedMarker([source, target], "source")!;
		expect(marker).toEqual({ way: "to", source: "source", other: target });
		expect(movedLine(marker)).toBe("Moved your message to Release notes");
		expect(movedAction(marker)).toBe("Keep here");
	});

	it("shows where it came from in the chat it went to", () => {
		const marker = movedMarker([source, target], "target")!;
		expect(marker).toEqual({ way: "from", source: "source", other: source });
		expect(movedLine(marker)).toBe("Moved your message here from Slow login");
		expect(movedAction(marker)).toBe("Move back");
	});

	it("shows nothing once you write again", () => {
		const settled = { ...source, moved_to: null };
		expect(movedMarker([settled, target], "source")).toBeNull();
		expect(movedMarker([settled, target], "target")).toBeNull();
		expect(movedMarker([source, target], null)).toBeNull();
	});
});

describe("a chat saved to Notes", () => {
	const notes = (list: NoteSummary[]): Entity => ({
		entity: "project_notes",
		project: "proj-1",
		device: "dev-mac",
		notes: list,
	});
	const note = (id: string, agent: string | null, deleted = false): NoteSummary => ({
		note: id,
		title: id,
		head: `hash-${id}`,
		at: NOW,
		by: { agent, device: "dev-mac" },
		created_on: "dev-mac",
		deleted,
		size: 10,
	});

	const chatAgent = (activity: Agent["activity"]): Entity => ({
		entity: "agent",
		...agent({ id: "agent-1", activity }),
	});
	const IDLE: Agent["activity"] = { state: "idle" };
	const BUSY: Agent["activity"] = { state: "busy" };

	it("is the new note the chat's agent wrote", () => {
		const save = startNoteSave(
			viewOf([chatAgent(IDLE), notes([note("old", "agent-1")])]),
			"dev-mac",
			"agent-1"
		);
		const after = viewOf([
			chatAgent(IDLE),
			notes([
				note("old", "agent-1"),
				note("other", "agent-2"),
				note("gone", "agent-1", true),
				note("saved", "agent-1"),
			]),
		]);
		expect(noteSaveStep(after, save)).toEqual({
			type: "saved",
			project: "proj-1",
			note: "saved",
		});
		expect(noteSaveStep(after, { ...save, chat: "agent-3" })).toEqual({ type: "ended" });
	});

	it("waits while the agent works or asks, and ends once it stops without a note", () => {
		const start = startNoteSave(viewOf([chatAgent(IDLE)]), "dev-mac", "agent-1");
		// Not started yet.
		expect(noteSaveStep(viewOf([chatAgent(IDLE)]), start)).toEqual({
			type: "saving",
			save: start,
		});
		const working = noteSaveStep(viewOf([chatAgent(BUSY)]), start);
		if (working.type !== "saving") throw new Error("Ended while working");
		expect(working.save.worked).toBe(true);
		const asking = viewOf([chatAgent({ state: "needs_input", attention: "att-1" })]);
		expect(noteSaveStep(asking, working.save).type).toBe("saving");
		expect(noteSaveStep(viewOf([chatAgent(IDLE)]), working.save)).toEqual({ type: "ended" });
	});
});

describe("acting on a chat", () => {
	const DONE: ActionResult = { status: "ok", data: { status: "done" } };
	const calls = (reply: ActionResult | Error) => {
		const made: unknown[][] = [];
		const answer = async (...args: unknown[]) => {
			made.push(args);
			if (reply instanceof Error) throw reply;
			return reply;
		};
		return { made, calls: { actOnChat: answer, deleteAgent: answer } };
	};
	const flowOf = () =>
		createDispatchFlow({
			askDispatcher: async () => DONE,
			confirmPlan: async () => DONE,
			dismissPlan: async () => DONE,
		});

	it("leaves a new conversation you started when you pick a chat", async () => {
		const flow = flowOf();
		startOver(flow);
		const { made, calls: on } = calls(DONE);
		expect(await actOnChat(flow, on, "dev-mac", "agent-1", { type: "pick" })).toBe(true);
		expect(made).toEqual([["dev-mac", "agent-1", { type: "pick" }]]);
		expect(flow.store.getState()).toMatchObject({ fresh: false, note: null });
	});

	it("keeps a new conversation through a rename, and deletes through the agent", async () => {
		const flow = flowOf();
		startOver(flow);
		const { made, calls: on } = calls(DONE);
		await actOnChat(flow, on, "dev-mac", "agent-1", { type: "rename", title: "Login" });
		await actOnChat(flow, on, "dev-mac", "agent-1", { type: "delete" });
		expect(made).toEqual([
			["dev-mac", "agent-1", { type: "rename", title: "Login" }],
			["agent-1"],
		]);
		expect(flow.store.getState().fresh).toBe(true);
	});

	it("shows why it didn't go as the flow's note", async () => {
		const flow = flowOf();
		const refused = calls({
			status: "ok",
			data: { status: "refused", reason: "that chat isn't here any more" },
		});
		expect(await actOnChat(flow, refused.calls, "dev-mac", "a", { type: "pick" })).toBe(false);
		expect(flow.store.getState().note).toBe("That chat isn't here any more.");

		const thrown = calls(new Error("socket closed"));
		await actOnChat(flow, thrown.calls, "dev-mac", "a", { type: "keep_here" });
		expect(flow.store.getState().note).toBe("Socket closed.");
	});
});
