// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Agent, AgentId, Inbox, InboxChange } from "../bindings";
import type { ActionResult, DataSource } from "../data/source";
import { agent, entities, NOW } from "../test/build";
import { DEFAULT_INBOX } from "../view/inbox";
import { applySnapshot } from "../view/replica";
import { createViewStore } from "../view/store";

import {
	changeInbox,
	createInboxNotices,
	UNDO_MS,
	undoNotice,
	watchAutoSettles,
} from "./inbox-notice";

const pinned: Inbox = { ...DEFAULT_INBOX, pin: { at: NOW, key: "m" } };
const auto: Inbox = { ...DEFAULT_INBOX, settled: { at: NOW, by: "auto" } };

const setup = (agents: Agent[], answer?: (id: AgentId) => ActionResult) => {
	const calls: [AgentId[], InboxChange][] = [];
	const source = {
		setInbox: async (ids: AgentId[], change: InboxChange) => {
			calls.push([ids, change]);
			return ids.map(
				(id) => answer?.(id) ?? ({ status: "ok", data: { status: "done" } } as const)
			);
		},
	} as unknown as DataSource;
	const view = createViewStore();
	const show = (list: Agent[]) =>
		view.setState({
			view: applySnapshot({ seq: 1, entities: entities({ agents: list }) }),
		});
	show(agents);
	const notices = createInboxNotices(source, view, () => NOW);
	return { calls, notices, show, state: () => notices.store.getState() };
};

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("a change you make", () => {
	it("says what it did and undoes it with each agent's own inbox", async () => {
		const { calls, notices, state } = setup([
			agent({ id: "a" }),
			agent({ id: "b", inbox: pinned }),
		]);
		await changeInbox(notices, [{ agents: ["a", "b"], change: { change: "settle" } }]);
		expect(state().notice?.text).toBe("Settled 2 agents");

		expect(undoNotice(notices)).toBe(true);
		await vi.runAllTimersAsync();
		expect(calls.slice(1)).toEqual([
			[["a"], { change: "restore", inbox: DEFAULT_INBOX }],
			[["b"], { change: "restore", inbox: pinned }],
		]);
		expect(state().notice).toBeNull();
		expect(undoNotice(notices)).toBe(false);
	});

	it("offers Undo for five seconds", async () => {
		const { notices, state } = setup([agent({ id: "a" })]);
		await changeInbox(notices, [{ agents: ["a"], change: { change: "settle" } }]);
		vi.advanceTimersByTime(UNDO_MS - 1);
		expect(state().notice).not.toBeNull();
		vi.advanceTimersByTime(1);
		expect(state().notice).toBeNull();
	});

	it("says why it failed for the agents it failed for", async () => {
		const { notices, state } = setup(
			[agent({ id: "a" }), agent({ id: "b", title: "Fix CI" })],
			(id) =>
				id === "b"
					? { status: "ok", data: { status: "refused", reason: "it works right now" } }
					: { status: "ok", data: { status: "done" } }
		);
		await changeInbox(notices, [{ agents: ["a", "b"], change: { change: "settle" } }]);
		expect(state().notice?.text).toBe("Settled a");
		expect(state().notice?.undo).toEqual([
			{ agents: ["a"], change: { change: "restore", inbox: DEFAULT_INBOX } },
		]);
		expect(state().failures).toEqual(["Couldn't settle Fix CI. It works right now."]);
	});
});

describe("auto-settles that arrive live", () => {
	it("leave a notice whose Undo brings the agents back", async () => {
		const { calls, notices, show, state } = setup([agent({ id: "a" }), agent({ id: "b" })]);
		const stop = watchAutoSettles(notices);
		show([agent({ id: "a", inbox: auto }), agent({ id: "b", inbox: auto })]);
		expect(state().notice?.text).toBe("Auto-settled 2 idle agents");
		undoNotice(notices);
		await vi.runAllTimersAsync();
		expect(calls).toEqual([[["a", "b"], { change: "unsettle" }]]);
		stop();
	});

	it("aren't taken for your own undo", async () => {
		const { notices, show, state } = setup([agent({ id: "a" })]);
		const stop = watchAutoSettles(notices);
		const change = changeInbox(notices, [
			{ agents: ["a"], change: { change: "restore", inbox: auto } },
		]);
		show([agent({ id: "a", inbox: auto })]);
		await change;
		expect(state().notice).toBeNull();
		stop();
	});
});
