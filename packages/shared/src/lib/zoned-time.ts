// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A time of day in a named zone, and a moment that shows it. A task that
// runs "at" a time keeps "HH:MM" in its own zone, which need not be the
// phone's, while a time picker works in moments. These turn one into the
// other, so a picker shows and sets the task's time on the task's clock.

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
	const match = /^(\d{1,2}):(\d{2})$/.exec(time.trim());
	const hour = Number(match?.[1]);
	const minute = Number(match?.[2]);
	if (!match || hour > 23 || minute > 59) return now;
	const known = knownZone(zone);
	const today = wallClock(now, known);
	const at = (day: number) => momentAt({ ...today, day, hour, minute }, known);
	// Date.UTC carries day 32 into the next month, so the next day needs no
	// calendar of its own.
	return at(today.day) ?? at(today.day + 1) ?? now;
};
