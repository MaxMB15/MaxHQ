// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { DeviceSettings, UsageWindow } from "../bindings";
import { mockSource } from "../data/mock-source";
import { CLAUDE, CODEX, CURSOR, DEVBOX_CLAUDE, DEVBOX_CODEX, MAC } from "../data/mock-view";
import { account, agent, entities, MIN, NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";
import { subscriptions } from "./subscriptions";
import { accountWindows, cycleWindows, FIVE_HOURS, WEEK } from "./usage";
import {
	accountStatus,
	meterRowLabel,
	planNote,
	resetText,
	resetTitle,
	usageCheckFailure,
	usageFailureNote,
	usagePlans,
	weekNote,
} from "./usage-text";

const load = async (): Promise<ViewState> =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());

const THU_9 = new Date(2026, 9, 29, 9, 0).getTime();

const window = (
	minutes: number,
	percent: number | null,
	resets_at: number | null
): UsageWindow => ({
	scope: null,
	window_minutes: minutes,
	used_percent: percent,
	resets_at,
});

const usage = (windows: UsageWindow[], checked_at = NOW - 20_000) => ({
	checked_at,
	source: "claude_probe" as const,
	windows,
});

describe("meter", () => {
	it("reads each meter row out", () => {
		const row = {
			vendor: "claude" as const,
			label: "Claude",
			percent: 22,
			usedUp: false,
			weekPercent: null,
		};
		expect(meterRowLabel(row)).toBe("Claude 22% used");
		expect(meterRowLabel({ ...row, percent: 40, weekPercent: 61.6 })).toBe(
			"Claude 40% used, 62% of the week"
		);
		expect(meterRowLabel({ ...row, percent: 99.2, usedUp: true })).toBe("Claude at its limit");
		expect(meterRowLabel({ ...row, percent: null })).toBe("Claude not checked yet");
	});

	it("names the account at its limit and the agents waiting on it", async () => {
		const view = await load();
		const [claude, codex] = usagePlans(view);
		expect(accountStatus(view, codex!)).toBe("At limit, 1 agent waiting");
		expect(accountStatus(view, claude!)).toBe("24 agents");
	});

	it("counts the agents of every login on the plan", async () => {
		const mac = account({ id: "mac" });
		const devbox = account({ id: "devbox", login: "setup_token", origin: "mac", email: null });
		const view = applySnapshot({
			seq: 1,
			entities: entities({
				accounts: [mac, devbox],
				agents: [
					agent({ id: "a", account: "mac" }),
					agent({ id: "b", account: "devbox" }),
					agent({ id: "c", account: "devbox" }),
				],
			}),
		});
		const [plan] = subscriptions([mac, devbox]);
		expect(accountStatus(view, plan!)).toBe("3 agents");
	});
});

describe("usage details", () => {
	it("explains a 5-hour window emptied by the week", () => {
		const windows = accountWindows({
			usage: usage([window(FIVE_HOURS, 12, NOW + 60 * MIN), window(WEEK, 99, THU_9)]),
		});
		expect(weekNote(windows, NOW)).toBe(
			"The week is used up, so the 5-hour window has nothing left until Thu 09:00."
		);
		const fine = accountWindows({
			usage: usage([window(FIVE_HOURS, 12, NOW + 60 * MIN), window(WEEK, 40, THU_9)]),
		});
		expect(weekNote(fine, NOW)).toBeUndefined();
	});

	it("says when each window resets", () => {
		const [five, week] = accountWindows({
			usage: usage([window(FIVE_HOURS, 30, NOW + 142 * MIN), window(WEEK, null, null)]),
		});
		expect(resetText(five!, NOW)).toBe("resets in 2h, 17:10");
		expect(resetText(week!, NOW)).toBe("Not reported");
	});

	it("counts down under a day and names the day further out, across midnight", () => {
		const late = new Date(2026, 9, 25, 22, 30).getTime();
		const resetAt = (at: number) =>
			resetText(accountWindows({ usage: usage([window(FIVE_HOURS, 30, at)]) })[0]!, late);
		// 01:15 is tomorrow, but under a day away, so it counts down.
		expect(resetAt(new Date(2026, 9, 26, 1, 15).getTime())).toBe("resets in 2h, 01:15");
		expect(resetAt(late + 20 * 1000)).toBe("resets in 1m, 22:30");
		expect(resetAt(late + 47 * MIN)).toBe("resets in 47m, 23:17");
		expect(resetAt(new Date(2026, 9, 27, 4, 30).getTime())).toBe("resets Tue 04:30");
		expect(resetAt(THU_9)).toBe("resets Thu 09:00");
		// A reset already past waits for the next reading rather than counting up.
		expect(resetAt(late - 5 * MIN)).toBe("resets 22:25");
	});

	it("gives the full day and time as the reset's tooltip", () => {
		const [five, week] = accountWindows({
			usage: usage([window(FIVE_HOURS, 30, NOW + 142 * MIN), window(WEEK, 10, null)]),
		});
		expect(resetTitle(five!)).toBe("Sunday, October 25 at 17:10");
		expect(resetTitle(week!)).toBeUndefined();
	});

	it("shows money spent where a spend limit would reset", () => {
		const [demand] = cycleWindows({
			usage: usage([
				{
					...window(0, 24.8, THU_9),
					scope: "on_demand",
					period: "billing_cycle",
					spend: { used_cents: 1240, limit_cents: 5000 },
				},
			]),
		});
		expect(resetText(demand!, NOW)).toBe("$12.40 of $50");
	});

	it("points Cursor at the device setting before its first reading", () => {
		const cursor = account({ id: "c", vendor: "cursor", usage: null, usage_failure: null });
		expect(usageFailureNote(cursor, NOW)).toBe(
			"Turn on Cursor usage in a Mac's device settings to see this month here."
		);
		expect(usageFailureNote({ ...cursor, usage_failure: { at: NOW, reason: "x" } }, NOW)).toBe(
			"Couldn't check usage. X."
		);
	});

	it("says an ACP agent doesn't report usage", () => {
		const gemini = account({ id: "g", vendor: "acp", usage: null, usage_failure: null });
		expect(usageFailureNote(gemini, NOW)).toBe("Doesn't report usage.");
		expect(usageFailureNote({ ...gemini, usage_failure: { at: NOW, reason: "x" } }, NOW)).toBe(
			"Couldn't check usage. X."
		);
	});

	it("says when the newest usage check failed or never ran", () => {
		const base = account({
			id: "a",
			vendor: "claude",
			plan: "max",
			usage: null,
			usage_failure: null,
		});
		expect(usageFailureNote(base, NOW)).toBe("Not checked yet.");
		expect(
			usageFailureNote(
				{ ...base, usage_failure: { at: NOW - MIN, reason: "logged out" } },
				NOW
			)
		).toBe("Couldn't check usage. Logged out.");

		const checked = { ...base, usage: usage([], NOW - 30 * MIN) };
		expect(usageFailureNote(checked, NOW)).toBeUndefined();
		expect(
			usageFailureNote(
				{ ...checked, usage_failure: { at: NOW - 2 * MIN, reason: "claude not found" } },
				NOW
			)
		).toBe("The last check 2 min ago failed. Claude not found. These figures are from 14:18.");
		expect(
			usageFailureNote(
				{ ...checked, usage_failure: { at: NOW - 40 * MIN, reason: "x" } },
				NOW
			)
		).toBeUndefined();
	});
});

describe("usageCheckFailure", () => {
	it("says why a check now didn't go out", () => {
		expect(usageCheckFailure({ status: "ok", data: null })).toBeNull();
		expect(
			usageCheckFailure({
				status: "error",
				error: { kind: "not_connected", device: "devbox" },
			})
		).toBe("Couldn't check usage. devbox isn't connected.");
	});
});

describe("usagePlans", () => {
	it("lists Claude and Codex once each, and Cursor once its switch is on", async () => {
		const source = mockSource({ now: () => NOW, tick: false });
		const before = applySnapshot(await source.snapshot());
		const plans = usagePlans(before);
		expect(plans.map((p) => p.account.id)).toEqual([CLAUDE, CODEX]);
		expect(plans.map((p) => p.logins.map((a) => a.id))).toEqual([
			[CLAUDE, DEVBOX_CLAUDE],
			[CODEX, DEVBOX_CODEX],
		]);

		const settings = before.tables.device_settings.get(MAC)!;
		const on: DeviceSettings = { ...settings, cursor_usage: true };
		await source.setDeviceSettings(on);
		const after = applySnapshot(await source.snapshot());
		expect(usagePlans(after).map((p) => p.account.id)).toEqual([CLAUDE, CODEX, CURSOR]);
	});
});

describe("planNote", () => {
	it("says where a token's usage shows when its own login isn't in sight", async () => {
		const view = await load();
		const token = view.tables.account.get(DEVBOX_CLAUDE)!;
		const signIn = view.tables.account.get(DEVBOX_CODEX)!;
		const [claude] = subscriptions([token]);
		const [codex] = subscriptions([signIn]);
		expect(planNote(view, claude!, NOW)).toBe(
			"Signed in on devbox with a token. Its usage shows with the login it came from."
		);
		expect(planNote(view, codex!, NOW)).toBe(
			"Signed in on devbox. Its usage shows with a Mac's Codex login on the same email."
		);
	});

	it("tells how the last check went otherwise", async () => {
		const view = await load();
		const [plan] = subscriptions([account({ id: "c" })]);
		expect(planNote(view, plan!, NOW)).toBe("Not checked yet.");
	});
});
