// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Agent, Entity, Inbox, UsageWindow } from "../bindings";
import { account, agent, entities, MIN, NOW, workspace } from "../test/build";

import { DEFAULT_INBOX } from "./inbox";
import {
	doneText,
	failureLines,
	inboxFailureOf,
	inboxMenu,
	inboxTargets,
	limitResetOf,
	limitSnoozedText,
	newlyAutoSettled,
	pinMoveSteps,
	restoreSteps,
} from "./inbox-actions";
import { applySnapshot, type ViewState } from "./replica";

const HOUR = 60 * MIN;

const inbox = (over: Partial<Inbox>): Inbox => ({ ...DEFAULT_INBOX, ...over });

const window = (minutes: number, used: number, resets: number): UsageWindow => ({
	window_minutes: minutes,
	used_percent: used,
	resets_at: resets,
	scope: null,
});

const viewOf = (agents: Agent[], accounts = [account({ id: "max" })]): ViewState =>
	applySnapshot({ seq: 1, entities: entities({ agents, accounts }) });

describe("the menu's switch for continuing after the limit", () => {
	const view = applySnapshot({
		seq: 1,
		entities: [
			...entities({
				agents: [
					agent({ id: "a", control: "held" }),
					agent({ id: "b", control: "held", vendor: "codex" }),
					agent({ id: "c", control: "held", inbox: inbox({ continue_at_limit: true }) }),
					agent({ id: "cursor", control: "held", vendor: "cursor" }),
				],
				workspaces: [workspace({ id: "w1" })],
			}),
			{
				entity: "device_settings",
				device: "dev-mac",
				default_permission: "auto",
				workspace_network: null,
				workspace_pool: null,
				continue_claude_at_limit: false,
				continue_codex_at_limit: true,
			} as Entity,
		],
	});
	const menu = (...ids: string[]) => inboxMenu(view, inboxTargets(view, ids), NOW);

	it("turns it on for an agent that doesn't carry on", () => {
		const one = menu("a");
		expect(one.continueAtLimit).toBe("off");
		expect(one.continueOff).toBeNull();
		expect(one.continueOn).toEqual({
			label: "On",
			steps: [{ agents: ["a"], change: { change: "continue_at_limit", on: true } }],
		});
	});

	it("counts the device default and the agent's own switch", () => {
		expect(menu("b").continueAtLimit).toBe("on");
		expect(menu("c").continueAtLimit).toBe("on");
		const mixed = menu("a", "b", "c");
		expect(mixed.continueAtLimit).toBe("mixed");
		expect(mixed.continueOn?.label).toBe("On (1)");
		expect(mixed.continueOff?.label).toBe("Off (2)");
	});

	it("isn't offered when one agent can't have it", () => {
		const cursor = menu("a", "cursor");
		expect(cursor.continueOn).toBeNull();
		expect(cursor.continueOff).toBeNull();
	});

	it("says what changed", () => {
		const a = view.tables.agent.get("a")!;
		expect(doneText({ change: "continue_at_limit", on: true }, [a], NOW)).toBe(
			"Continue after the limit on for a"
		);
	});
});

describe("the menu", () => {
	const view = viewOf([
		agent({ id: "a", account: "max" }),
		agent({ id: "b", account: "max", inbox: inbox({ pin: { at: NOW, key: "m" } }) }),
		agent({ id: "c", inbox: inbox({ settled: { at: NOW, by: "you" } }) }),
		agent({ id: "d", inbox: inbox({ pin: { at: NOW, key: "x" }, auto_settle_off: true }) }),
		agent({ id: "e", inbox: inbox({ snoozed_until: NOW + HOUR }) }),
		agent({ id: "sub", parent: "a" }),
	]);
	const menu = (...ids: string[]) => inboxMenu(view, inboxTargets(view, ids), NOW);

	it("leaves sub-agents out, since they go with their lead", () => {
		expect(inboxTargets(view, ["sub", "a", "gone"]).map((a) => a.id)).toEqual(["a"]);
	});

	it("names one agent's choices plainly", () => {
		const one = menu("a");
		expect(one.settle?.label).toBe("Settle");
		expect(one.pin?.label).toBe("Pin");
		expect(one.snooze).toEqual(["a"]);
		expect(one.wake).toBeNull();
		expect(one.autoSettle).toBe("on");
		expect(one.autoSettleOn).toBeNull();
		expect(one.autoSettleOff?.label).toBe("Off");
	});

	it("settles only the unsettled ones and counts them", () => {
		const several = menu("a", "c");
		expect(several.settle).toEqual({
			label: "Settle (1)",
			steps: [{ agents: ["a"], change: { change: "settle" } }],
		});
		expect(menu("c").settle?.label).toBe("Unsettle");
	});

	it("pins the unpinned ones after every pin there is", () => {
		const pin = menu("a", "b").pin!;
		expect(pin.label).toBe("Pin (1)");
		const change = pin.steps[0]!.change;
		expect(change.change === "pin" && change.key > "x").toBe(true);
		expect(menu("b", "d").pin?.label).toBe("Unpin (2)");
	});

	it("offers Wake only when every one is snoozed", () => {
		expect(menu("e").wake?.label).toBe("Wake");
		expect(menu("e").snooze).toBeNull();
		expect(menu("a", "e").snooze).toEqual(["a", "e"]);
	});

	it("shows a mixed auto-settle as neither", () => {
		const mixed = menu("a", "d");
		expect(mixed.autoSettle).toBe("mixed");
		expect(mixed.autoSettleOn?.steps[0]?.agents).toEqual(["d"]);
		expect(mixed.autoSettleOff?.steps[0]?.agents).toEqual(["a"]);
	});
});

describe("until the limit resets", () => {
	const weekly = account({
		id: "max",
		usage: {
			source: "claude_cache",
			checked_at: NOW,
			windows: [window(300, 40, NOW + HOUR), window(7 * 24 * 60, 99, NOW + 30 * HOUR)],
		},
	});
	const plus = account({
		id: "plus",
		vendor: "codex",
		plan: "plus",
		usage: {
			checked_at: NOW,
			source: "codex_app_server",
			windows: [window(300, 100, NOW + 2 * HOUR)],
		},
	});
	const roomy = account({
		id: "roomy",
		usage: { checked_at: NOW, source: "claude_cache", windows: [window(300, 98, NOW + HOUR)] },
	});
	const view = viewOf(
		[
			agent({ id: "on-max", account: "max" }),
			agent({ id: "on-plus", account: "plus" }),
			agent({ id: "roomy", account: "roomy" }),
			agent({
				id: "waiting",
				activity: { state: "waiting_for_limit_reset", resets_at: NOW + 3 * HOUR },
			}),
		],
		[weekly, plus, roomy]
	);
	const at = (id: string) => limitResetOf(view, view.tables.agent.get(id)!, NOW);

	it("is the reset of the used-up window that frees up last", () => {
		expect(at("on-max")).toBe(NOW + 30 * HOUR);
		expect(at("on-plus")).toBe(NOW + 2 * HOUR);
		expect(at("waiting")).toBe(NOW + 3 * HOUR);
	});

	it("isn't offered below the limit", () => {
		expect(at("roomy")).toBeNull();
		const menu = inboxMenu(view, inboxTargets(view, ["on-max", "roomy"]), NOW);
		expect(menu.untilLimit).toBeNull();
	});

	it("snoozes each account's agents to that account's reset", () => {
		const menu = inboxMenu(view, inboxTargets(view, ["on-max", "on-plus"]), NOW);
		expect(menu.untilLimit?.steps).toEqual([
			{ agents: ["on-max"], change: { change: "snooze", until: NOW + 30 * HOUR } },
			{ agents: ["on-plus"], change: { change: "snooze", until: NOW + 2 * HOUR } },
		]);
	});
});

describe("the notice", () => {
	const a = agent({ id: "a", title: "Release checklist" });
	const b = agent({ id: "b" });

	it("names one agent and counts several", () => {
		expect(doneText({ change: "settle" }, [a], NOW)).toBe("Settled Release checklist");
		expect(doneText({ change: "settle" }, [a, b], NOW)).toBe("Settled 2 agents");
		expect(doneText({ change: "snooze", until: NOW + HOUR }, [a, b], NOW)).toBe(
			"Snoozed 2 agents until 15:48"
		);
		expect(doneText({ change: "auto_settle", on: false }, [a], NOW)).toBe(
			"Auto-settle off for Release checklist"
		);
		expect(doneText({ change: "dismiss_woke" }, [a], NOW)).toBeNull();
		expect(limitSnoozedText([a, b])).toBe("Snoozed 2 agents until the limit resets");
	});

	it("calls a pin on a pinned agent a move", () => {
		const pinned = agent({ id: "p", inbox: inbox({ pin: { at: NOW, key: "m" } }) });
		expect(doneText({ change: "pin", key: "c" }, [pinned], NOW)).toBe("Moved p");
		expect(doneText({ change: "pin", key: "c" }, [a], NOW)).toBe("Pinned Release checklist");
	});
});

describe("moving a pin", () => {
	const pinnedAt = (key: string) => inbox({ pin: { at: NOW, key } });
	const view = viewOf([
		agent({ id: "a", inbox: pinnedAt("c") }),
		agent({ id: "b", inbox: pinnedAt("m") }),
		agent({ id: "c", inbox: pinnedAt("t") }),
	]);
	const keyOf = (steps: ReturnType<typeof pinMoveSteps>) => {
		const change = steps[0]?.change;
		return change?.change === "pin" ? change.key : null;
	};

	it("gives the moved pin one key between its new neighbors", () => {
		const top = pinMoveSteps(view, ["a", "b", "c"], "c", 0);
		expect(top.map((step) => step.agents)).toEqual([["c"]]);
		expect(keyOf(top)! < "c").toBe(true);

		const bottom = pinMoveSteps(view, ["a", "b", "c"], "a", 3);
		expect(keyOf(bottom)! > "t").toBe(true);

		const middle = pinMoveSteps(view, ["a", "b", "c"], "a", 2);
		expect(keyOf(middle)! > "m" && keyOf(middle)! < "t").toBe(true);
	});

	it("makes no call when the pin lands where it was", () => {
		expect(pinMoveSteps(view, ["a", "b", "c"], "b", 1)).toEqual([]);
		expect(pinMoveSteps(view, ["a", "b", "c"], "b", 2)).toEqual([]);
		expect(pinMoveSteps(view, ["a", "b"], "c", 0)).toEqual([]);
	});

	it("rewrites the shown pins when a key next to it is corrupt", () => {
		const broken = viewOf([
			agent({ id: "x", inbox: pinnedAt("a") }),
			agent({ id: "y", inbox: pinnedAt("m") }),
		]);
		const steps = pinMoveSteps(broken, ["x", "y"], "y", 0);
		const keys = new Map(
			steps.map((step) => [
				step.agents[0],
				step.change.change === "pin" ? step.change.key : "",
			])
		);
		expect(keys.size).toBe(2);
		expect(keys.get("y")! < keys.get("x")!).toBe(true);
	});
});

describe("undo", () => {
	it("restores each agent's own inbox, one call for each that differs", () => {
		const pinned = inbox({ pin: { at: NOW, key: "m" } });
		const steps = restoreSteps([
			agent({ id: "a" }),
			agent({ id: "b", inbox: pinned }),
			agent({ id: "c", inbox: DEFAULT_INBOX }),
		]);
		expect(steps).toEqual([
			{ agents: ["a", "c"], change: { change: "restore", inbox: DEFAULT_INBOX } },
			{ agents: ["b"], change: { change: "restore", inbox: pinned } },
		]);
	});
});

describe("auto-settles that arrive live", () => {
	const settled = inbox({ settled: { at: NOW - 6 * 24 * HOUR, by: "auto" } });

	it("are the agents auto-settle settled since the last view", () => {
		const before = viewOf([agent({ id: "a" }), agent({ id: "b" }), agent({ id: "c" })]);
		const after = viewOf([
			agent({ id: "a", inbox: settled }),
			agent({ id: "b", inbox: inbox({ settled: { at: NOW, by: "you" } }) }),
			agent({ id: "c" }),
			agent({ id: "new", inbox: settled }),
		]);
		expect(newlyAutoSettled(before, after).map((a) => a.id)).toEqual(["a"]);
		expect(newlyAutoSettled(after, after)).toEqual([]);
	});
});

describe("failures", () => {
	it("reads refusals and errors in words", () => {
		expect(
			inboxFailureOf({
				status: "ok",
				data: { status: "refused", reason: "it needs input first" },
			})
		).toBe("It needs input first");
		expect(
			inboxFailureOf({
				status: "error",
				error: { kind: "not_connected", device: "devbox" },
			})
		).toBe("devbox isn't connected");
		expect(inboxFailureOf({ status: "ok", data: { status: "done" } })).toBeNull();
	});

	it("gives one sentence for each reason", () => {
		const old = "devbox's maxd is too old to settle, snooze or pin its agents. Update it first";
		const lines = failureLines({ change: "settle" }, [
			{ agent: agent({ id: "a" }), reason: old },
			{ agent: agent({ id: "b" }), reason: old },
			{ agent: agent({ id: "c", title: "Fix CI" }), reason: "It works right now" },
		]);
		expect(lines).toEqual([
			`Couldn't settle 2 agents. ${old}.`,
			"Couldn't settle Fix CI. It works right now.",
		]);
	});
});
