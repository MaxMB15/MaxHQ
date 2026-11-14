// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { InlineRef, TimelineItem } from "../bindings";
import { mockSource } from "../data/mock-source";
import { CLAUDE, MAC } from "../data/mock-view";
import type { ActionResult } from "../data/source";
import { agent, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { conversationItems, isWorking } from "./conversation";
import {
	ask,
	confirm,
	createDispatchFlow,
	dismiss,
	type DispatchFlow,
	hasPanel,
	inUse,
	setDraft,
	shownPlan,
	SIGN_IN_NOTE,
	startOver,
	talkingTo,
} from "./flow";
import { resultRows, resultsTitle } from "./plan";

const REQUEST = "Review octocat's new PRs, 15 minutes after the last one";
const TARGET = { device: MAC, account: CLAUDE };

/** The flow over the sample data, and what the view shows of the Mac's dispatcher. */
const sample = () => {
	const mock = mockSource({ now: () => NOW, tick: false });
	const flow = createDispatchFlow(mock);
	const view = async () => applySnapshot(await mock.snapshot());
	const shown = async () => {
		const state = await view();
		const dispatcher = state.tables.dispatcher.get(MAC);
		const now = flow.store.getState();
		const talking = talkingTo(state, now, dispatcher);
		return { state, talking, plan: shownPlan(now, dispatcher) };
	};
	/** Asks, then answers the question with `project`. */
	const planFor = async (project: string) => {
		setDraft(flow, REQUEST);
		await ask(flow, TARGET);
		const { talking } = await shown();
		if (talking?.activity.state !== "needs_input") throw new Error("No question");
		await mock.answer(talking.activity.attention, { type: "option", id: project });
		return (await shown()).plan!;
	};
	return { mock, flow, shown, planFor };
};

/** A flow over calls that all come back with `reply`, and the calls made. */
const replying = (reply: ActionResult | Error) => {
	const calls: unknown[][] = [];
	const call = async (...args: unknown[]) => {
		calls.push(args);
		if (reply instanceof Error) throw reply;
		return reply;
	};
	const flow = createDispatchFlow({ askDispatcher: call, confirmPlan: call, dismissPlan: call });
	return { flow, calls };
};

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

const PLAN = { id: 3, items: [] };

const PILL: InlineRef = {
	type: "agent",
	agent: "agent-2",
	title: "Release notes",
	vendor: "claude",
	device: MAC,
	excerpt: null,
};

const state = (flow: DispatchFlow) => flow.store.getState();

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("asking the dispatcher", () => {
	it("sends what you typed and clears the box", async () => {
		const { flow, shown } = sample();
		setDraft(flow, `  ${REQUEST} `);
		expect(await ask(flow, TARGET)).toBe(true);
		expect(state(flow)).toMatchObject({ text: "", busy: false, note: null, fresh: false });
		const { talking } = await shown();
		expect(talking?.title).toBe("Dispatcher");
		expect(inUse(state(flow), talking, null)).toBe(true);
	});

	it("sends nothing while the box is blank or a call is on its way", async () => {
		const { flow, calls } = replying({ status: "ok", data: { status: "done" } });
		setDraft(flow, "   ");
		expect(await ask(flow, TARGET)).toBe(false);
		setDraft(flow, REQUEST);
		flow.store.setState({ busy: true });
		expect(await ask(flow, TARGET)).toBe(false);
		expect(calls).toEqual([]);
	});

	it("sends your pills to a maxd that takes them, and clears them with the box", async () => {
		const { flow, calls } = replying({ status: "ok", data: { status: "done" } });
		setDraft(flow, "Ship it like \uFFFC ", [PILL]);
		expect(await ask(flow, { ...TARGET, pills: true })).toBe(true);
		expect(calls).toEqual([[MAC, "Ship it like \uFFFC", CLAUDE, false, [PILL]]]);
		expect(state(flow)).toMatchObject({ text: "", refs: [] });
	});

	it("sends an older maxd each pill as the words it shows", async () => {
		const { flow, calls } = replying({ status: "ok", data: { status: "done" } });
		setDraft(flow, "Ship it like \uFFFC", [PILL]);
		await ask(flow, TARGET);
		expect(calls).toEqual([[MAC, "Ship it like [Agent: Release notes]", CLAUDE, false, []]]);
	});

	it("asks you to sign in when the Mac has no account for it", async () => {
		const { flow, calls } = replying({ status: "ok", data: { status: "done" } });
		setDraft(flow, REQUEST);
		expect(await ask(flow, { device: MAC, account: null })).toBe(false);
		expect(state(flow).note).toBe(SIGN_IN_NOTE);
		expect(calls).toEqual([]);
	});

	it("keeps what you typed and says why when it doesn't go", async () => {
		const { flow } = replying(refused("the dispatcher runs on the Mac"));
		setDraft(flow, REQUEST);
		expect(await ask(flow, TARGET)).toBe(false);
		expect(state(flow)).toMatchObject({
			text: REQUEST,
			busy: false,
			note: "The dispatcher runs on the Mac.",
		});
		const thrown = replying(new Error("the phone lost its core"));
		setDraft(thrown.flow, REQUEST);
		await ask(thrown.flow, TARGET);
		expect(state(thrown.flow)).toMatchObject({
			busy: false,
			note: "The phone lost its core.",
		});
	});

	it("starts a new conversation with your next request once you start over", async () => {
		const { flow, calls } = replying({ status: "ok", data: { status: "done" } });
		flow.store.setState({ note: "Old.", ran: { plan: PLAN, results: [] } });
		startOver(flow);
		expect(state(flow)).toMatchObject({ fresh: true, note: null, ran: null });
		const dispatcher = { device: MAC, workspace: "w", agent: "a1", plan: PLAN };
		const view = applySnapshot({ seq: 1, entities: [] });
		expect(talkingTo(view, state(flow), dispatcher)).toBeUndefined();
		expect(shownPlan(state(flow), dispatcher)).toBeNull();
		expect(hasPanel(state(flow), undefined)).toBe(true);
		setDraft(flow, REQUEST);
		await ask(flow, TARGET);
		expect(calls).toEqual([[MAC, REQUEST, CLAUDE, true, []]]);
		expect(state(flow).fresh).toBe(false);
	});
});

describe("a plan", () => {
	it("waits on you, then runs and says how each item went", async () => {
		const { flow, shown, planFor } = sample();
		const plan = await planFor("Comet");
		const before = await shown();
		expect(inUse(state(flow), before.talking, before.plan)).toBe(true);
		expect(await confirm(flow, MAC, plan)).toBe(true);
		const { ran } = state(flow);
		expect(ran?.plan).toBe(plan);
		expect(resultsTitle(ran!.results)).toBe("The plan ran");
		const after = await shown();
		const rows = resultRows(after.state, plan, ran!.results);
		expect(rows.map((r) => r.line)).toEqual(["Started", "Task saved"]);
		expect(rows[0]!.phrase).toBe("A Cursor agent in Comet on Mac");
		expect(after.state.tables.agent.get(rows[0]!.agent!)?.title).toBe("PR reviews");
		expect(rows[1]).toMatchObject({ agent: null, task: "Review octocat's PRs" });
		expect(after.plan).toBeNull();
		expect(inUse(state(flow), after.talking, after.plan)).toBe(false);
		expect(hasPanel(state(flow), after.talking)).toBe(true);
	});

	it("drops a plan you dismiss", async () => {
		const { flow, shown, planFor } = sample();
		const plan = await planFor("MaxHQ");
		expect(await dismiss(flow, MAC, plan)).toBe(true);
		expect(state(flow)).toMatchObject({ busy: false, note: null, ran: null });
		expect((await shown()).plan).toBeNull();
	});

	it("says why a plan didn't run", async () => {
		const done = replying({ status: "ok", data: { status: "done" } });
		expect(await confirm(done.flow, MAC, PLAN)).toBe(false);
		expect(state(done.flow).note).toBe("The plan didn't run.");
		const away = replying({ status: "error", error: { kind: "not_connected", device: MAC } });
		await confirm(away.flow, MAC, PLAN);
		expect(state(away.flow).note).toBe("The device isn't connected, so nothing changed.");
		const stale = replying(refused("That plan isn't the one shown any more."));
		await dismiss(stale.flow, MAC, PLAN);
		expect(state(stale.flow).note).toBe("That plan isn't the one shown any more.");
	});

	it("does nothing without a plan or a device", async () => {
		const { flow, calls } = replying({ status: "ok", data: { status: "done" } });
		expect(await confirm(flow, MAC, null)).toBe(false);
		expect(await dismiss(flow, null, PLAN)).toBe(false);
		expect(calls).toEqual([]);
	});

	it("marks the item that stopped a plan", () => {
		const view = applySnapshot({ seq: 1, entities: [] });
		const plan = {
			id: 1,
			items: [
				{
					reason: "Asks",
					step: { type: "send_message" as const, device: MAC, agent: "a", text: "Hi" },
				},
				{
					reason: "Asks again",
					step: { type: "send_message" as const, device: MAC, agent: "a", text: "Hi" },
				},
			],
		};
		const results = [{ type: "failed" as const, reason: "No such agent." }];
		expect(resultsTitle(results)).toBe("The plan stopped at item 1");
		expect(resultRows(view, plan, results)).toEqual([
			{
				phrase: "A message to an agent",
				line: "No such agent.",
				failed: true,
				agent: null,
				task: null,
			},
			{
				phrase: "A message to an agent",
				line: "Not run, an earlier item failed",
				failed: false,
				agent: null,
				task: null,
			},
		]);
	});
});

describe("the conversation", () => {
	it("shows what was said and asked, not the reading", () => {
		const said = (id: string, kind: TimelineItem["kind"]) =>
			({ id, kind }) as unknown as TimelineItem;
		const items = new Map(
			[
				said("1", "user_message"),
				said("2", "file_read"),
				said("3", "question"),
				said("4", "command"),
				said("5", "assistant_message"),
			].map((i) => [i.id, i])
		);
		expect(conversationItems(["1", "2", "3", "4", "5", "6"], items).map((i) => i.id)).toEqual([
			"1",
			"3",
			"5",
		]);
	});

	it("is working while the agent starts or works", () => {
		expect(isWorking(agent({ id: "a", activity: { state: "busy" } }))).toBe(true);
		expect(isWorking(agent({ id: "a", activity: { state: "starting" } }))).toBe(true);
		expect(isWorking(agent({ id: "a", activity: { state: "idle" } }))).toBe(false);
	});
});
