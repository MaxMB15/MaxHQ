// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the snooze cases, over
// millisecond timestamps. The editor's helpers are tested here too.

import { afterEach, describe, expect, it, vi } from "vitest";

import {
	DEFAULT_SNOOZE_PRESETS,
	isSnoozePresetList,
	isSnoozePresetSetting,
	nextSnoozeCandidate,
	pickSnoozePreset,
	resolveCustomSnooze,
	resolveSnoozePresets,
	sameSnoozePresets,
	snoozeMenuLine,
	snoozeRowNotes,
	snoozeWakeLabel,
	sortSnoozeRows,
	splitSnoozeMinutes,
	type SnoozePresetSetting,
	withSnoozeWhen,
} from "./snooze";

const localDate = (year: number, month: number, day: number, hour: number, minute = 0): Date =>
	new Date(year, month - 1, day, hour, minute, 0, 0);

describe("the snooze menu", () => {
	const formatTime = (date: Date) =>
		`${date.getHours()}:${String(date.getMinutes()).padStart(2, "0")}`;
	const menu = (now: Date, presets?: readonly SnoozePresetSetting[]) =>
		resolveSnoozePresets(now, { formatTime, ...(presets ? { presets } : {}) }).map(
			(preset) => `${preset.label} ${preset.whenLabel}`
		);

	it("lists options soonest first and names the day when the label does not", () => {
		// Wednesday 20:00.
		expect(
			menu(localDate(2026, 4, 8, 20), [
				{ kind: "at", day: "monday", time: "18:00" },
				{ kind: "at", day: "tomorrow", time: "21:00" },
				{ kind: "after", minutes: 360 },
				{ kind: "after", minutes: 180 },
			])
		).toEqual([
			"In 3 hours 23:00",
			"In 6 hours Thu 2:00",
			"Tomorrow 21:00",
			"Next week Mon 18:00",
		]);
	});

	it("drops a today option once its time passes and moves a weekday on that day to next week", () => {
		// Monday 18:30.
		const now = localDate(2026, 4, 13, 18, 30);
		const presets = resolveSnoozePresets(now, {
			presets: [
				{ kind: "at", day: "today", time: "18:00" },
				{ kind: "at", day: "monday", time: "09:00" },
			],
		});
		expect(presets.map((preset) => preset.id)).toEqual(["at:monday:09:00"]);
		expect(presets[0]!.until).toBe(localDate(2026, 4, 20, 9).getTime());
	});

	it("names the real day gap: Monday on a Sunday is Tomorrow, a weekday a week out is Next", () => {
		const presets: readonly SnoozePresetSetting[] = [
			{ kind: "at", day: "tomorrow", time: "21:00" },
			{ kind: "at", day: "monday", time: "18:00" },
			{ kind: "at", day: "friday", time: "17:00" },
		];
		// Sunday 14:00.
		expect(menu(localDate(2026, 8, 30, 14), presets)).toEqual([
			"Tomorrow 18:00",
			"Tomorrow 21:00",
			"Friday 17:00",
		]);
		// Friday 10:00: today's 17:00 is a week away.
		expect(menu(localDate(2026, 9, 4, 10), presets)).toEqual([
			"Tomorrow 21:00",
			"Next week Mon 18:00",
			"Next Friday 17:00",
		]);
	});

	it("shows one row when two options wake at the same time", () => {
		// Sunday, so Tomorrow 9:00 and Next week 9:00 are the same Monday.
		expect(menu(localDate(2026, 8, 30, 7, 1))).toEqual([
			"In 1 hour 8:01",
			"In 3 hours 10:01",
			"This evening 18:00",
			"Tomorrow 9:00",
		]);
	});

	it("treats a duration and a set time in the same minute as one row", () => {
		const now = localDate(2026, 4, 8, 15);
		now.setSeconds(25);
		expect(
			menu(now, [
				{ kind: "after", minutes: 180 },
				{ kind: "at", day: "today", time: "18:00" },
			])
		).toEqual(["In 3 hours 18:00"]);
	});
});

describe("picking an option", () => {
	it("starts a duration when it is picked, even when the menu merged it with a set time", () => {
		const presets: readonly SnoozePresetSetting[] = [
			{ kind: "at", day: "today", time: "18:30" },
			{ kind: "after", minutes: 180 },
		];
		// At 15:00 the two differ; at 15:30 "In 3 hours" lands on 18:30 and the menu shows one row.
		const shown = resolveSnoozePresets(localDate(2026, 4, 8, 15), { presets });
		const picked = pickSnoozePreset({
			shown,
			id: "after:180",
			now: localDate(2026, 4, 8, 15, 30),
			presets,
		});
		expect(picked?.until).toBe(localDate(2026, 4, 8, 18, 30).getTime());
	});

	it("keeps a set time's shown wake when the day changes while the menu is open", () => {
		// Sunday 23:59: "Tomorrow" means Monday 9:00, even if the pick lands after midnight.
		const shown = resolveSnoozePresets(localDate(2026, 8, 30, 23, 59));
		const picked = pickSnoozePreset({
			shown,
			id: "at:tomorrow:09:00",
			now: localDate(2026, 8, 31, 0, 1),
		});
		expect(picked?.until).toBe(localDate(2026, 8, 31, 9).getTime());
	});

	it("gives nothing once a set time has passed", () => {
		const shown = resolveSnoozePresets(localDate(2026, 4, 8, 17, 59));
		const picked = pickSnoozePreset({
			shown,
			id: "at:today:18:00",
			now: localDate(2026, 4, 8, 18, 1),
		});
		expect(picked).toBeUndefined();
	});
});

describe("custom snooze", () => {
	const now = new Date(2026, 8, 14, 14, 30);

	afterEach(() => vi.unstubAllEnvs());

	it.each([
		["2026-09-14", "14:30"],
		["2027-02-29", "09:00"],
		["2026-09-15", "24:00"],
	])("rejects invalid or non-future calendar input %s %s", (date, time) => {
		expect(resolveCustomSnooze({ mode: "date", date, time }, now)).toBeNull();
	});

	it.each([
		["minutes", "45", 45 * 60_000],
		["hours", "1.5", 90 * 60_000],
		["days", "2", 48 * 3_600_000],
	] as const)("resolves %s from the confirmation time", (unit, amount, elapsed) => {
		const confirmedAt = new Date(now.getTime() + 5 * 60_000);
		expect(resolveCustomSnooze({ mode: "duration", amount, unit }, confirmedAt)).toBe(
			confirmedAt.getTime() + elapsed
		);
	});

	it.each(["0", "NaN", "1e300"])("rejects invalid duration %s", (amount) => {
		expect(resolveCustomSnooze({ mode: "duration", amount, unit: "hours" }, now)).toBeNull();
	});

	it("rejects nonexistent local times at the spring DST transition", () => {
		vi.stubEnv("TZ", "America/Los_Angeles");
		expect(
			resolveCustomSnooze(
				{ mode: "date", date: "2027-03-14", time: "02:30" },
				new Date("2027-03-01T00:00:00Z")
			)
		).toBeNull();
	});

	it("treats duration days as 24 hours across DST", () => {
		vi.stubEnv("TZ", "America/Los_Angeles");
		const before = new Date(2027, 2, 13, 12);
		const wake = resolveCustomSnooze({ mode: "duration", amount: "1", unit: "days" }, before);
		expect(wake).toBe(new Date(2027, 2, 14, 13).getTime());
	});
});

describe("the wake label", () => {
	it("counts down in minutes, hours and days", () => {
		const now = localDate(2026, 4, 8, 12);
		expect(snoozeWakeLabel(now.getTime() - 1, now.getTime())).toBe("now");
		expect(snoozeWakeLabel(now.getTime() + 25 * 60_000, now.getTime())).toBe("25m");
		expect(snoozeWakeLabel(now.getTime() + 150 * 60_000, now.getTime())).toBe("3h");
		expect(snoozeWakeLabel(now.getTime() + 30 * 3_600_000, now.getTime())).toBe("2d");
	});
});

describe("the options editor", () => {
	it("orders rows like the menu, with a passed today option last", () => {
		const presets: readonly SnoozePresetSetting[] = [
			{ kind: "at", day: "today", time: "09:00" },
			{ kind: "at", day: "tomorrow", time: "09:00" },
			{ kind: "after", minutes: 60 },
		];
		const sorted = sortSnoozeRows(presets, [0, 1, 2], localDate(2026, 4, 8, 12));
		expect(sorted.keys).toEqual([2, 1, 0]);
	});

	it("only takes options that are well formed", () => {
		expect(isSnoozePresetSetting({ kind: "after", minutes: 60 })).toBe(true);
		expect(isSnoozePresetSetting({ kind: "at", day: "friday", time: "17:00" })).toBe(true);
		expect(isSnoozePresetSetting({ kind: "after", minutes: 0 })).toBe(false);
		expect(isSnoozePresetSetting({ kind: "after", minutes: 1.5 })).toBe(false);
		expect(isSnoozePresetSetting({ kind: "at", day: "someday", time: "17:00" })).toBe(false);
		expect(isSnoozePresetSetting({ kind: "at", day: "today", time: "25:00" })).toBe(false);
		expect(isSnoozePresetSetting(null)).toBe(false);
	});

	it("only takes a whole menu of good options, eight at most", () => {
		expect(isSnoozePresetList(DEFAULT_SNOOZE_PRESETS)).toBe(true);
		expect(isSnoozePresetList([])).toBe(true);
		const nine = Array.from({ length: 9 }, (_, i) => ({ kind: "after", minutes: i + 1 }));
		expect(isSnoozePresetList(nine)).toBe(false);
		expect(isSnoozePresetList([{ kind: "after", minutes: 60 }, { kind: "after" }])).toBe(false);
		expect(isSnoozePresetList({ kind: "after", minutes: 60 })).toBe(false);
	});
});

describe("the options editor's rows", () => {
	const formatTime = (date: Date) =>
		`${date.getHours()}:${String(date.getMinutes()).padStart(2, "0")}`;

	it("says why a row makes no menu row", () => {
		// Wednesday 12:00, so today at 9:00 has passed and in 6 hours is 18:00.
		const notes = snoozeRowNotes(
			[
				{ kind: "after", minutes: 60 },
				{ kind: "at", day: "today", time: "09:00" },
				{ kind: "after", minutes: 60 },
				{ kind: "at", day: "today", time: "18:00" },
				{ kind: "after", minutes: 360 },
			],
			localDate(2026, 4, 8, 12),
			formatTime
		);
		expect(notes).toEqual([
			{ shown: true, label: "In 1 hour", when: "13:00" },
			{ shown: false, text: "Passed for today" },
			{ shown: false, text: "Duplicate of an option above" },
			{ shown: true, label: "This evening", when: "18:00" },
			{ shown: false, text: "Same time as an option above" },
		]);
	});

	it("names the menu, with the time where two rows read alike", () => {
		const now = localDate(2026, 4, 8, 12);
		expect(snoozeMenuLine(DEFAULT_SNOOZE_PRESETS, now, formatTime)).toBe(
			"Menu now: In 1 hour · In 3 hours · This evening · Tomorrow · Next week"
		);
		expect(
			snoozeMenuLine(
				[
					{ kind: "at", day: "tomorrow", time: "09:00" },
					{ kind: "at", day: "tomorrow", time: "14:00" },
				],
				now,
				formatTime
			)
		).toBe("Menu now: Tomorrow 9:00 · Tomorrow 14:00");
		expect(snoozeMenuLine([{ kind: "at", day: "today", time: "09:00" }], now)).toBe(
			"Menus offer only Custom…"
		);
	});

	it("adds the first option the list lacks", () => {
		expect(nextSnoozeCandidate([])).toEqual({ kind: "after", minutes: 60 });
		expect(nextSnoozeCandidate(DEFAULT_SNOOZE_PRESETS)).toEqual({ kind: "after", minutes: 30 });
	});

	it("counts the same options in another order as the same", () => {
		const shuffled = [...DEFAULT_SNOOZE_PRESETS].reverse();
		expect(sameSnoozePresets(shuffled, DEFAULT_SNOOZE_PRESETS)).toBe(true);
		expect(sameSnoozePresets(shuffled.slice(1), DEFAULT_SNOOZE_PRESETS)).toBe(false);
	});

	it("shows a duration in its largest even unit", () => {
		expect(splitSnoozeMinutes(2 * 24 * 60)).toEqual({ amount: 2, unit: "days" });
		expect(splitSnoozeMinutes(180)).toEqual({ amount: 3, unit: "hours" });
		expect(splitSnoozeMinutes(90)).toEqual({ amount: 90, unit: "minutes" });
	});

	it("keeps the other kind's values when When changes", () => {
		const memory = { minutes: 120, time: "07:30" };
		const after: SnoozePresetSetting = { kind: "after", minutes: 60 };
		const at: SnoozePresetSetting = { kind: "at", day: "friday", time: "17:00" };
		expect(withSnoozeWhen(after, "monday", memory)).toEqual({
			kind: "at",
			day: "monday",
			time: "07:30",
		});
		expect(withSnoozeWhen(at, "tomorrow", memory)).toEqual({
			kind: "at",
			day: "tomorrow",
			time: "17:00",
		});
		expect(withSnoozeWhen(at, "after", memory)).toEqual({ kind: "after", minutes: 120 });
		expect(withSnoozeWhen(after, "after", memory)).toBe(after);
	});
});
