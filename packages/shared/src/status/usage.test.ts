// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { UsageWindow } from "../bindings";
import { account, NOW } from "../test/build";

import {
	accountName,
	accountSummary,
	accountWindows,
	atLimit,
	blockingWindow,
	cycleWindows,
	isUsedUp,
	meterRows,
	shortWindow,
	windowShort,
} from "./usage";

const HOUR = 3_600_000;

const win = (
	window_minutes: number,
	used_percent: number | null,
	resets_at: number | null = null
): UsageWindow => ({
	scope: null,
	window_minutes,
	used_percent,
	resets_at,
});

const withWindows = (id: string, windows: UsageWindow[], vendor: "claude" | "codex" = "claude") =>
	account({
		id,
		vendor,
		plan: vendor === "claude" ? "max" : "plus",
		usage: {
			checked_at: NOW,
			source: vendor === "claude" ? "claude_cache" : "codex_app_server",
			windows,
		},
	});

describe("isUsedUp", () => {
	it("counts 99% and above as used up", () => {
		expect(isUsedUp({ used_percent: 98.9 })).toBe(false);
		expect(isUsedUp({ used_percent: 99 })).toBe(true);
		expect(isUsedUp({ used_percent: 100 })).toBe(true);
		expect(isUsedUp({ used_percent: null })).toBe(false);
	});
});

describe("accountWindows", () => {
	it("names the windows and sorts them shortest first", () => {
		const a = withWindows("a", [win(10080, 45), win(300, 22)]);
		expect(accountWindows(a).map((w) => [w.label, w.percent, w.usedUp])).toEqual([
			["5 hours", 22, false],
			["Week", 45, false],
		]);
	});

	it("shows nothing left in the 5-hour window once the week is used up", () => {
		const weekReset = NOW + 50 * HOUR;
		const a = withWindows("a", [win(300, 12, NOW + HOUR), win(10080, 99.4, weekReset)]);
		const five = shortWindow(a)!;
		expect(five.percent).toBe(100);
		expect(five.usedUp).toBe(true);
		expect(five.blockedByWeek).toBe(true);
		expect(five.resetsAt).toBe(weekReset);
		expect(blockingWindow(a)?.label).toBe("Week");
	});

	it("leaves the week alone when only the 5-hour window is used up", () => {
		const a = withWindows("a", [win(300, 100, NOW + HOUR), win(10080, 64)]);
		expect(accountWindows(a).map((w) => w.usedUp)).toEqual([true, false]);
		expect(atLimit(a)).toBe(true);
		expect(blockingWindow(a)?.label).toBe("5 hours");
	});

	it("leaves out windows that cover only part of the account", () => {
		const a = withWindows("a", [win(300, 10), { ...win(10080, 100), scope: "opus" }]);
		expect(accountWindows(a)).toHaveLength(1);
		expect(atLimit(a)).toBe(false);
	});

	it("shows nothing before the first reading", () => {
		expect(accountWindows(account({ id: "a" }))).toEqual([]);
		expect(shortWindow(account({ id: "a" }))).toBeUndefined();
	});
});

describe("cycleWindows", () => {
	const cycle = (scope: string | null, used_percent: number): UsageWindow => ({
		...win(0, used_percent, NOW + 300 * HOUR),
		scope,
		period: "billing_cycle",
	});

	it("names the month meters in the order maxd sent them, apart from the rolling windows", () => {
		const a = withWindows("a", [
			cycle(null, 38),
			cycle("auto", 24),
			cycle("api", 61),
			{ ...cycle("on_demand", 24.8), spend: { used_cents: 1240, limit_cents: 5000 } },
		]);
		expect(cycleWindows(a).map((w) => [w.label, w.percent, w.resetsAt])).toEqual([
			["Total", 38, NOW + 300 * HOUR],
			["Auto", 24, NOW + 300 * HOUR],
			["API", 61, NOW + 300 * HOUR],
			["On-demand", 24.8, NOW + 300 * HOUR],
		]);
		expect(cycleWindows(a)[3]!.spend).toEqual({ used_cents: 1240, limit_cents: 5000 });
		expect(accountWindows(a)).toEqual([]);
	});

	it("never puts the account at its limit", () => {
		const a = withWindows("a", [cycle(null, 100)]);
		expect(cycleWindows(a)[0]!.usedUp).toBe(true);
		expect(atLimit(a)).toBe(false);
		expect(blockingWindow(a)).toBeUndefined();
	});

	it("leaves rolling windows to accountWindows", () => {
		expect(cycleWindows(withWindows("a", [win(300, 10)]))).toEqual([]);
	});
});

describe("windowShort", () => {
	it("names a window by its length", () => {
		expect([300, 10080, 1440, 60, 45].map(windowShort)).toEqual([
			"5h",
			"wk",
			"1d",
			"1h",
			"45m",
		]);
	});
});

describe("accountName", () => {
	it("names the plan the CLI reported", () => {
		expect(accountName({ vendor: "claude", label: "Claude", plan: "max" })).toBe("Claude Max");
		expect(accountName({ vendor: "codex", label: "Codex", plan: "plus" })).toBe("Codex Plus");
	});

	it("never guesses a plan it wasn't told", () => {
		expect(accountName({ vendor: "claude", label: "Claude", plan: null })).toBe("Claude");
		expect(accountName({ vendor: "codex", label: "Codex", plan: " " })).toBe("Codex");
	});

	it("calls a local ACP agent by the name you gave it", () => {
		expect(accountName({ vendor: "acp", label: "Gemini", plan: null })).toBe("Gemini");
	});
});

describe("meterRows", () => {
	it("shows one row per vendor with the rules applied", () => {
		const claude = withWindows("c", [win(300, 22), win(10080, 45)]);
		const codex = withWindows("x", [win(300, 40), win(10080, 99)], "codex");
		expect(meterRows([claude, codex])).toEqual([
			{ vendor: "claude", label: "Claude", percent: 22, usedUp: false, weekPercent: 45 },
			{ vendor: "codex", label: "Codex", percent: 100, usedUp: true, weekPercent: 99 },
		]);
	});

	it("takes the week from the account whose 5 hours it shows", () => {
		const quiet = withWindows("a", [win(300, 10), win(10080, 90)]);
		const busy = {
			...withWindows("b", [win(300, 60), win(10080, 30)]),
			email: "b@example.com",
		};
		expect(meterRows([quiet, busy])[0]?.weekPercent).toBe(30);
		expect(meterRows([busy, quiet])[0]?.weekPercent).toBe(30);
	});

	it("shows no week when the account has no weekly window or no reading", () => {
		expect(meterRows([withWindows("c", [win(300, 40)])])[0]?.weekPercent).toBeNull();
		expect(
			meterRows([withWindows("c", [win(300, 40), win(10080, null)])])[0]?.weekPercent
		).toBeNull();
	});

	it("leaves out vendors with no account and shows no figure before a reading", () => {
		expect(meterRows([account({ id: "c" })])).toEqual([
			{ vendor: "claude", label: "Claude", percent: null, usedUp: false, weekPercent: null },
		]);
	});
});

describe("accountSummary", () => {
	it("says how much of the 5 hours is used, or that it's at its limit", () => {
		expect(accountSummary(withWindows("c", [win(300, 62)]))).toBe("Claude Max, 62% of 5 hours");
		expect(accountSummary(withWindows("x", [win(300, 99)], "codex"))).toBe(
			"Codex Plus, at its 5-hour limit"
		);
		expect(accountSummary(withWindows("x", [win(300, 3), win(10080, 100)], "codex"))).toBe(
			"Codex Plus, at its weekly limit"
		);
	});
});
