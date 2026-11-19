// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A time of day in a named zone, and a moment that shows it. A task that
// runs "at" a time keeps "HH:MM" in its own zone, which need not be the
// phone's, while a time picker works in moments. These turn one into the
// other, and say how far apart two zones' clocks are, so a picker on the
// phone's clock can show and set a time kept on the task's.

/** `zone` when this device knows it, or undefined for the device's own zone. */
export const knownZone = (zone: string): string | undefined => {
	try {
		new Intl.DateTimeFormat("en-US", { timeZone: zone });
		return zone;
	} catch {
		return undefined;
	}
};

/** What a wall clock shows, to the minute. */
type WallClock = {
	readonly year: number;
	readonly month: number;
	readonly day: number;
	readonly hour: number;
	readonly minute: number;
};

/** What the clocks in `zone`, or the device's when undefined, show at `date`. */
const wallClock = (date: Date, zone: string | undefined): WallClock => {
	const parts = new Intl.DateTimeFormat("en-US", {
		timeZone: zone,
		hourCycle: "h23",
		year: "numeric",
		month: "numeric",
		day: "numeric",
		hour: "numeric",
		minute: "numeric",
	}).formatToParts(date);
	const part = (type: Intl.DateTimeFormatPartTypes) =>
		Number(parts.find((each) => each.type === type)?.value ?? 0);
	return {
		year: part("year"),
		month: part("month"),
		day: part("day"),
		hour: part("hour") % 24,
		minute: part("minute"),
	};
};

/** `wall` read as if it were UTC, in milliseconds. */
const asUtc = (wall: WallClock): number =>
	Date.UTC(wall.year, wall.month - 1, wall.day, wall.hour, wall.minute);

/**
 * The moment the clocks in `zone` show `wall`, or null when they skip it,
 * as on the night they move forward. The zone's offset is read at a first
 * guess, then again where that lands, since the two differ near a change.
 */
const momentAt = (wall: WallClock, zone: string | undefined): Date | null => {
	const target = asUtc(wall);
	let guess = target;
	for (let pass = 0; pass < 2; pass++)
		guess = target - (asUtc(wallClock(new Date(guess), zone)) - guess);
	return asUtc(wallClock(new Date(guess), zone)) === target ? new Date(guess) : null;
};

const pad = (n: number) => String(n).padStart(2, "0");

/** "HH:MM" as an hour and a minute, or null when it isn't a time of day. */
export const parseTime = (text: string): { hour: number; minute: number } | null => {
	const match = /^(\d{1,2}):(\d{2})$/.exec(text.trim());
	const hour = Number(match?.[1]);
	const minute = Number(match?.[2]);
	return match && hour <= 23 && minute <= 59 ? { hour, minute } : null;
};

/**
 * How many minutes the clocks in `zone`, or the device's when undefined,
 * are ahead of UTC at `at`.
 */
const offsetAt = (at: Date, zone: string | undefined): number => {
	const minute = Math.floor(at.getTime() / 60_000) * 60_000;
	return (asUtc(wallClock(at, zone)) - minute) / 60_000;
};

/**
 * How many minutes the clocks in `to` are ahead of those in `from` at
 * `at`, behind when it's below zero. A zone this device doesn't know reads
 * as its own.
 */
export const clockGap = (from: string, to: string, at: Date): number =>
	offsetAt(at, knownZone(to)) - offsetAt(at, knownZone(from));

/** The time of day `date` shows in `zone`, as "HH:MM". */
export const timeOfDay = (date: Date, zone: string): string => {
	const { hour, minute } = wallClock(date, knownZone(zone));
	return `${pad(hour)}:${pad(minute)}`;
};

/**
 * A moment that shows `time` ("HH:MM") in `zone` on the day it is there at
 * `now`, or on the next day when the clocks skip it today. Gives `now` back
 * when `time` isn't a time of day.
 */
export const dateAtTime = (time: string, zone: string, now: Date): Date => {
	const parsed = parseTime(time);
	if (!parsed) return now;
	const { hour, minute } = parsed;
	const known = knownZone(zone);
	const today = wallClock(now, known);
	const at = (day: number) => momentAt({ ...today, day, hour, minute }, known);
	// Date.UTC carries day 32 into the next month, so the next day needs no
	// calendar of its own.
	return at(today.day) ?? at(today.day + 1) ?? now;
};
