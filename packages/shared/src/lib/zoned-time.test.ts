// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { clockGap, dateAtTime, knownZone, parseTime, timeOfDay } from "./zoned-time";

describe("zoned time", () => {
	it("reads the time of day on the zone's clock", () => {
		const moment = new Date("2026-06-01T07:05:00Z");
		expect(timeOfDay(moment, "Europe/Amsterdam")).toBe("09:05");
		expect(timeOfDay(moment, "Asia/Tokyo")).toBe("16:05");
		expect(timeOfDay(moment, "America/New_York")).toBe("03:05");
		expect(timeOfDay(new Date("2026-06-01T00:00:00Z"), "UTC")).toBe("00:00");
	});

	it("finds the moment a time shows on the zone's clock today", () => {
		const now = new Date("2026-06-01T12:00:00Z");
		expect(dateAtTime("09:00", "Europe/Amsterdam", now).toISOString()).toBe(
			"2026-06-01T07:00:00.000Z"
		);
		expect(dateAtTime("9:30", "America/New_York", now).toISOString()).toBe(
			"2026-06-01T13:30:00.000Z"
		);
		// Already June 2 in Tokyo.
		expect(
			dateAtTime("08:00", "Asia/Tokyo", new Date("2026-06-01T20:00:00Z")).toISOString()
		).toBe("2026-06-01T23:00:00.000Z");
	});

	it("uses the zone's offset on that day, not today's here", () => {
		expect(
			dateAtTime("09:00", "Europe/Amsterdam", new Date("2026-01-15T12:00:00Z")).toISOString()
		).toBe("2026-01-15T08:00:00.000Z");
	});

	it("moves to the next day when the clocks skip the time today", () => {
		// Amsterdam goes from 02:00 to 03:00 on 2026-03-29.
		const skipped = dateAtTime("02:30", "Europe/Amsterdam", new Date("2026-03-29T10:00:00Z"));
		expect(skipped.toISOString()).toBe("2026-03-30T00:30:00.000Z");
		expect(timeOfDay(skipped, "Europe/Amsterdam")).toBe("02:30");
	});

	it("finds the next day in a zone that skips midnight", () => {
		// Santiago goes from 00:00 to 01:00 on 2026-09-06.
		const skipped = dateAtTime("00:30", "America/Santiago", new Date("2026-09-06T12:00:00Z"));
		expect(skipped.toISOString()).toBe("2026-09-07T03:30:00.000Z");
	});

	it("round trips every time of day", () => {
		const now = new Date("2026-06-01T12:00:00Z");
		for (const zone of ["Europe/Amsterdam", "Asia/Kolkata", "America/Los_Angeles"])
			for (const time of ["00:00", "06:15", "12:30", "23:59"])
				expect(timeOfDay(dateAtTime(time, zone, now), zone)).toBe(time);
	});

	it("gives now back for a time that isn't one", () => {
		const now = new Date("2026-06-01T12:00:00Z");
		expect(dateAtTime("", "UTC", now)).toBe(now);
		expect(dateAtTime("24:00", "UTC", now)).toBe(now);
		expect(dateAtTime("9", "UTC", now)).toBe(now);
	});

	it("reads a time of day", () => {
		expect(parseTime("09:05")).toEqual({ hour: 9, minute: 5 });
		expect(parseTime(" 9:30 ")).toEqual({ hour: 9, minute: 30 });
		expect(parseTime("23:59")).toEqual({ hour: 23, minute: 59 });
		for (const text of ["", "24:00", "12:60", "9", "09:5", "noon"])
			expect(parseTime(text)).toBeNull();
	});

	it("says how far apart two zones' clocks are on the day", () => {
		const june = new Date("2026-06-10T12:00:00Z");
		expect(clockGap("America/New_York", "Europe/Amsterdam", june)).toBe(6 * 60);
		expect(clockGap("Europe/Amsterdam", "America/New_York", june)).toBe(-6 * 60);
		expect(clockGap("Europe/Amsterdam", "Asia/Kolkata", june)).toBe(3.5 * 60);
		expect(clockGap("UTC", "UTC", june)).toBe(0);
		// New York is on summer time from 2026-03-08, Amsterdam from 2026-03-29.
		const march = new Date("2026-03-20T12:00:00Z");
		expect(clockGap("America/New_York", "Europe/Amsterdam", march)).toBe(5 * 60);
		// Seconds into the minute don't count.
		expect(clockGap("UTC", "Asia/Tokyo", new Date("2026-06-10T12:00:42.500Z"))).toBe(9 * 60);
	});

	it("falls back to the device's zone for one it doesn't know", () => {
		expect(knownZone("Europe/Amsterdam")).toBe("Europe/Amsterdam");
		expect(knownZone("Mars/Olympus_Mons")).toBeUndefined();
		const moment = new Date("2026-06-01T07:05:00Z");
		const local = new Intl.DateTimeFormat("en-GB", {
			hour: "2-digit",
			minute: "2-digit",
			hourCycle: "h23",
		}).format(moment);
		expect(timeOfDay(moment, "Mars/Olympus_Mons")).toBe(local);
	});
});
