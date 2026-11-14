// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Entity } from "@maxhq/shared/bindings";
import { mockSource } from "@maxhq/shared/data/mock-source";
import { CLAUDE, CODEX, MAC } from "@maxhq/shared/data/mock-view";
import {
	ask,
	confirm,
	createDispatchFlow,
	dismiss,
	setDraft,
	startOver,
} from "@maxhq/shared/dispatch/flow";
import { resultRows } from "@maxhq/shared/dispatch/plan";
import { NOW } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import {
	BAR_SETTINGS,
	barSettingLabel,
	DEFAULT_PREFS,
	type DispatcherPrefs,
	dispatcherBar,
	readPrefs,
	resultLine,
	sheetOpens,
} from "./bar";

const ALWAYS: DispatcherPrefs = DEFAULT_PREFS;
const IN_USE: DispatcherPrefs = { bar: "in_use", account: null };
const REQUEST = "Review octocat's new PRs, 15 minutes after the last one";

// The phone reaches the sample's Mac over SSH, not on the same machine.
const overSsh = (entities: Entity[]): Entity[] =>
	entities.map((e) =>
		e.entity === "device_link" && e.device === MAC
			? { ...e, route: { type: "ssh", host: "example-user@devbox" } }
			: e
	);

/** The sample data as the phone sees it, with `change` made to it, and the dispatch flow over it. */
const sample = (change: (entities: Entity[]) => Entity[] = (e) => e) => {
	const mock = mockSource({ now: () => NOW, tick: false });
	const flow = createDispatchFlow(mock);
	const view = async () => {
		const snapshot = await mock.snapshot();
		return applySnapshot({ ...snapshot, entities: change(overSsh(snapshot.entities)) });
	};
	/** The bar over the view now, with `prefs`, and whether the sheet is open. */
	const bar = async (prefs: DispatcherPrefs, open = false) =>
		dispatcherBar(await view(), flow.store.getState(), prefs, open);
	/** Asks, then answers the dispatcher's question with `project`. */
	const planFor = async (project: string) => {
		setDraft(flow, REQUEST);
		await ask(flow, { device: MAC, account: CLAUDE });
		const { talking } = await bar(IN_USE);
		if (talking?.activity.state !== "needs_input") throw new Error("No question");
		await mock.answer(talking.activity.attention, { type: "option", id: project });
		return (await bar(IN_USE)).plan!;
	};
	return { mock, flow, view, bar, planFor };
};

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("the setting", () => {
	it("always shows the bar unless you picked otherwise", () => {
		expect(readPrefs(null)).toEqual({ bar: "always", account: null });
		expect(readPrefs("not json")).toEqual(DEFAULT_PREFS);
		expect(readPrefs("[1]")).toEqual(DEFAULT_PREFS);
		expect(readPrefs('{"bar":"sideways","account":7}')).toEqual(DEFAULT_PREFS);
		expect(readPrefs(JSON.stringify({ bar: "in_use", account: CODEX }))).toEqual({
			bar: "in_use",
			account: CODEX,
		});
	});

	it("names its two choices", () => {
		expect(BAR_SETTINGS.map((s) => s.label)).toEqual(["Always show", "Only when in use"]);
		expect(barSettingLabel("in_use")).toBe("Only when in use");
	});
});

describe("the bar", () => {
	it("always shows when you keep it, and takes what you type", async () => {
		const { flow, bar } = sample();
		const idle = await bar(ALWAYS);
		expect(idle).toMatchObject({ shows: "bar", inUse: false, canSend: false, status: null });
		expect(idle.reach).toMatchObject({ device: MAC, off: null });
		expect(idle.reach.account?.id).toBe(CLAUDE);
		setDraft(flow, REQUEST);
		expect((await bar(ALWAYS)).canSend).toBe(true);
	});

	it("runs as the account you picked while the Mac has it", async () => {
		const { bar } = sample();
		expect((await bar({ bar: "always", account: CODEX })).reach.account?.id).toBe(CODEX);
		expect((await bar({ bar: "always", account: "gone" })).reach.account?.id).toBe(CLAUDE);
	});

	it("folds to a grabber when kept for use, and shows while the sheet is open or you type", async () => {
		const { flow, bar } = sample();
		expect((await bar(IN_USE)).shows).toBe("grabber");
		expect((await bar(IN_USE, true)).shows).toBe("bar");
		setDraft(flow, "Review");
		expect((await bar(IN_USE)).shows).toBe("bar");
	});

	it("shows while the dispatcher works or asks, and folds back once the plan runs", async () => {
		const { flow, bar, planFor } = sample();
		setDraft(flow, REQUEST);
		await ask(flow, { device: MAC, account: CLAUDE });
		const asking = await bar(IN_USE);
		expect(asking).toMatchObject({
			shows: "bar",
			inUse: true,
			status: "The dispatcher asks you something",
		});
		expect(asking.talking?.title).toBe("Dispatcher");
		startOver(flow);
		expect((await bar(IN_USE)).shows).toBe("grabber");

		const plan = await planFor("Comet");
		expect(await bar(IN_USE)).toMatchObject({
			shows: "bar",
			inUse: true,
			status: "A plan waits on you",
		});
		expect(await confirm(flow, MAC, plan)).toBe(true);
		expect(await bar(IN_USE)).toMatchObject({ shows: "grabber", inUse: false, plan: null });
	});

	it("folds back once you dismiss the plan", async () => {
		const { flow, bar, planFor } = sample();
		const plan = await planFor("MaxHQ");
		expect(await dismiss(flow, MAC, plan)).toBe(true);
		expect(await bar(IN_USE)).toMatchObject({ shows: "grabber", plan: null });
	});
});

describe("the sheet", () => {
	it("shows the plan, and once you confirm, how each item went", async () => {
		const { flow, view, bar, planFor } = sample();
		const plan = await planFor("Comet");
		expect((await bar(ALWAYS, true)).plan).toBe(plan);
		await confirm(flow, MAC, plan);
		const { ran } = flow.store.getState();
		const rows = resultRows(await view(), ran!.plan, ran!.results);
		expect(rows.map(resultLine)).toEqual(["Started", "Task saved as Review octocat's PRs"]);
		expect(rows[0]!.agent).not.toBeNull();
		expect(rows[1]!.agent).toBeNull();
	});

	it("opens past halfway or on a flick up, and shuts the same way down", () => {
		expect(sheetOpens(false, -300, 0, 500)).toBe(true);
		expect(sheetOpens(false, -200, 0, 500)).toBe(false);
		expect(sheetOpens(false, -40, -1.2, 500)).toBe(true);
		expect(sheetOpens(true, 200, 0, 500)).toBe(true);
		expect(sheetOpens(true, 300, 0, 500)).toBe(false);
		expect(sheetOpens(true, 30, 1.2, 500)).toBe(false);
	});
});

describe("a dispatcher out of reach", () => {
	const unreachable = (entities: Entity[]): Entity[] =>
		entities.map((e) =>
			e.entity === "device_link" && e.device === MAC
				? { ...e, state: { state: "unreachable", reason: "timed out", last_seen: NOW } }
				: e
		);

	it("turns the bar off and says the Mac can't be reached", async () => {
		const { flow, bar } = sample(unreachable);
		setDraft(flow, REQUEST);
		const off = await bar(IN_USE);
		expect(off.reach.off).toBe("Can't reach Mac");
		expect(off.canSend).toBe(false);
		expect(off.shows).toBe("bar");
	});

	it("asks for a newer MaxHQ on a Mac that has no dispatcher", async () => {
		const { bar } = sample((entities) => entities.filter((e) => e.entity !== "dispatcher"));
		const off = await bar(ALWAYS);
		expect(off).toMatchObject({ shows: "bar", canSend: false, talking: undefined, plan: null });
		expect(off.reach.off).toBe("Update MaxHQ on Mac to use the dispatcher");
	});

	it("asks you to sign in on a Mac with no Claude or Codex account", async () => {
		const { bar } = sample((entities) => entities.filter((e) => e.entity !== "account"));
		expect((await bar(ALWAYS)).reach.off).toBe("Sign in to Claude or Codex on Mac");
	});
});
