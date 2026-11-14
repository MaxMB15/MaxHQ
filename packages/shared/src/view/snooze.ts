// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the snooze menu's options
// and the custom snooze, the preset setting and its defaults, and the
// sorting of snooze rows and the options editor's helpers. Rewritten as
// arrow functions over millisecond timestamps instead of ISO strings, with a
// plain check in place of a schema library.

import { t, formatDate } from "../i18n";
import { plural } from "../status/format";

import type { Timestamp } from "../bindings";

const HOUR_MS = 60 * 60 * 1_000;
const DAY_MS = 24 * HOUR_MS;

/** The longest an "after" option may wait, 30 days. */
export const MAX_SNOOZE_PRESET_MINUTES = 30 * 24 * 60;

/** How many options the snooze menu may have. */
export const MAX_SNOOZE_PRESETS = 8;

/** The days an "at" option can name. A weekday is its next one after today. */
export const SNOOZE_PRESET_DAYS = [
	"today",
	"tomorrow",
	"monday",
	"tuesday",
	"wednesday",
	"thursday",
	"friday",
	"saturday",
	"sunday",
] as const;

/** A day an "at" option can name. */
export type SnoozePresetDay = (typeof SNOOZE_PRESET_DAYS)[number];

/** One option in the snooze menu: a time from now, or a local time on a day. */
export type SnoozePresetSetting =
	| { readonly kind: "after"; readonly minutes: number }
	| { readonly kind: "at"; readonly day: SnoozePresetDay; readonly time: string };

/** The options the menu starts with. */
export const DEFAULT_SNOOZE_PRESETS: readonly SnoozePresetSetting[] = [
	{ kind: "after", minutes: 60 },
	{ kind: "after", minutes: 180 },
	{ kind: "at", day: "today", time: "18:00" },
	{ kind: "at", day: "tomorrow", time: "09:00" },
	{ kind: "at", day: "monday", time: "09:00" },
];

const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;

/** Whether a stored value is a snooze option, so a bad one is never offered. */
export const isSnoozePresetSetting = (value: unknown): value is SnoozePresetSetting => {
	if (typeof value !== "object" || value === null) return false;
	const preset = value as Record<string, unknown>;
	if (preset.kind === "after") {
		return (
			typeof preset.minutes === "number" &&
			Number.isInteger(preset.minutes) &&
			preset.minutes >= 1 &&
			preset.minutes <= MAX_SNOOZE_PRESET_MINUTES
		);
	}
	return (
		preset.kind === "at" &&
		(SNOOZE_PRESET_DAYS as readonly unknown[]).includes(preset.day) &&
		typeof preset.time === "string" &&
		TIME.test(preset.time)
	);
};

const WEEKDAY_INDEX = {
	sunday: 0,
	monday: 1,
	tuesday: 2,
	wednesday: 3,
	thursday: 4,
	friday: 5,
	saturday: 6,
} as const;

/** One row of the snooze menu, worked out for now. */
export type SnoozePreset = {
	/** The option it comes from, as `snoozePresetId` writes it. */
	readonly id: string;
	/** Such as "In 3 hours" or "Tomorrow". */
	readonly label: string;
	/** The wake time, with the day when the label doesn't name it. */
	readonly whenLabel: string;
	/** When the agent comes back. */
	readonly until: Timestamp;
};

/** What the menu rows are worked out from. */
export type SnoozePresetOptions = {
	readonly presets?: readonly SnoozePresetSetting[];
	readonly formatTime?: (date: Date) => string;
};

const snoozeTimeOfDayLabel = (date: Date): string =>
	formatDate(date, { hour: "numeric", minute: "2-digit" });

// Calendar-day advance instead of adding DAY_MS: fixed millisecond offsets
// land on the wrong local day across DST transitions (a spring-forward day
// is 23 hours, so 23:30 + 24h skips the whole next day).
const addSnoozeDays = (base: Date, days: number): Date => {
	const next = new Date(base.getTime());
	next.setDate(next.getDate() + days);
	return next;
};

const pluralize = (count: number, unit: string): string => plural(count, unit);

/** "3 hours", "90 minutes", "2 days": the largest unit that divides evenly. */
const formatSnoozeDuration = (minutes: number): string => {
	if (minutes % (24 * 60) === 0) return pluralize(minutes / (24 * 60), "day");
	if (minutes % 60 === 0) return pluralize(minutes / 60, "hour");
	return pluralize(minutes, "minute");
};

const capitalize = (word: string): string => t(word[0]!.toUpperCase() + word.slice(1));

const snoozePresetDaysAhead = (
	setting: Extract<SnoozePresetSetting, { kind: "at" }>,
	now: Date
): number => {
	if (setting.day === "today") return 0;
	if (setting.day === "tomorrow") return 1;
	return (WEEKDAY_INDEX[setting.day] - now.getDay() + 7) % 7 || 7;
};

/** Labels follow the actual day gap, so a Monday option on a Sunday reads "Tomorrow". */
const snoozePresetLabel = (setting: SnoozePresetSetting, daysAhead: number): string => {
	if (setting.kind === "after")
		return t("In {{formatSnoozeDuration_setting_minutes}}", {
			formatSnoozeDuration_setting_minutes: formatSnoozeDuration(setting.minutes),
		});
	if (daysAhead === 0) {
		const hour = Number(setting.time.slice(0, 2));
		return hour < 12 ? t("This morning") : hour < 17 ? t("This afternoon") : t("This evening");
	}
	if (daysAhead === 1) return t("Tomorrow");
	if (setting.day === "monday") return t("Next week");
	return daysAhead === 7
		? t("Next {{capitalize_setting_day}}", { capitalize_setting_day: capitalize(setting.day) })
		: capitalize(setting.day);
};

/** The option's stable name, such as "after:60" or "at:tomorrow:09:00". */
export const snoozePresetId = (setting: SnoozePresetSetting): string =>
	setting.kind === "after" ? `after:${setting.minutes}` : `at:${setting.day}:${setting.time}`;

/** One option worked out for now. Null for a today option whose time has passed. */
export const resolveSnoozePreset = (
	setting: SnoozePresetSetting,
	now: Date,
	formatTime: (date: Date) => string = snoozeTimeOfDayLabel
): SnoozePreset | null => {
	let wake: Date;
	let daysAhead = 0;
	if (setting.kind === "after") {
		wake = new Date(now.getTime() + setting.minutes * 60_000);
	} else {
		daysAhead = snoozePresetDaysAhead(setting, now);
		wake = addSnoozeDays(now, daysAhead);
		wake.setHours(Number(setting.time.slice(0, 2)), Number(setting.time.slice(3)), 0, 0);
		// A today option whose time has passed is left out until tomorrow.
		if (wake.getTime() <= now.getTime()) return null;
	}
	const label = snoozePresetLabel(setting, daysAhead);
	// Durations and "Next week" don't name the wake day, so a wake on another day shows it.
	const labelNamesDay = setting.kind === "at" && !(setting.day === "monday" && daysAhead > 1);
	const time = formatTime(wake);
	return {
		id: snoozePresetId(setting),
		label,
		whenLabel:
			labelNamesDay || wake.toDateString() === now.toDateString()
				? time
				: t("{{day}} {{time}}", { day: formatDate(wake, { weekday: "short" }), time }),
		until: wake.getTime(),
	};
};

/** The snooze menu: soonest first, one row per wake minute. */
export const resolveSnoozePresets = (
	now: Date,
	options: SnoozePresetOptions = {}
): readonly SnoozePreset[] => {
	// By minute, so a duration (which keeps the clock's seconds) and a set time in the same
	// minute tie, and the stable sort keeps whichever comes first in settings.
	const wakeMinute = (preset: SnoozePreset) => Math.floor(preset.until / 60_000);
	const seen = new Set<number>();
	return (options.presets ?? DEFAULT_SNOOZE_PRESETS)
		.flatMap((setting) => resolveSnoozePreset(setting, now, options.formatTime) ?? [])
		.sort((a, b) => wakeMinute(a) - wakeMinute(b))
		.filter((preset) => {
			if (seen.has(wakeMinute(preset))) return false;
			seen.add(wakeMinute(preset));
			return true;
		});
};

/**
 * A menu choice as picked. A duration re-resolves from its setting (not the
 * deduped menu) so it starts at selection. A set time keeps the shown wake,
 * which a day change would otherwise shift. Undefined once a set time has
 * passed.
 */
export const pickSnoozePreset = (input: {
	readonly shown: readonly SnoozePreset[];
	readonly id: string;
	readonly now: Date;
	readonly presets?: readonly SnoozePresetSetting[];
	readonly formatTime?: (date: Date) => string;
}): SnoozePreset | undefined => {
	const original = input.shown.find((preset) => preset.id === input.id);
	const ahead = original && original.until > input.now.getTime() ? original : undefined;
	const setting = (input.presets ?? DEFAULT_SNOOZE_PRESETS).find(
		(candidate) => candidate.kind === "after" && snoozePresetId(candidate) === input.id
	);
	return (setting && resolveSnoozePreset(setting, input.now, input.formatTime)) ?? ahead;
};

/** How long until a snooze ends, as "25m", "3h" or "2d". */
export const snoozeWakeLabel = (until: Timestamp, now: Timestamp): string => {
	const remaining = until - now;
	if (remaining <= 0) return t("now");
	if (remaining < HOUR_MS) return `${Math.max(1, Math.ceil(remaining / 60_000))}m`;
	if (remaining < DAY_MS) return `${Math.ceil(remaining / HOUR_MS)}h`;
	return `${Math.ceil(remaining / DAY_MS)}d`;
};

/** What the custom snooze dialog holds: a date and time, or a duration. */
export type CustomSnoozeInput =
	| { readonly mode: "date"; readonly date: string; readonly time: string }
	| {
			readonly mode: "duration";
			readonly amount: string;
			readonly unit: "minutes" | "hours" | "days";
	  };

/** When a custom snooze ends, or null when the input isn't a time in the future. */
export const resolveCustomSnooze = (input: CustomSnoozeInput, now: Date): Timestamp | null => {
	let wake: Date;
	if (input.mode === "duration") {
		const amount = Number(input.amount);
		if (!Number.isFinite(amount) || amount <= 0) return null;
		const unitMs = { minutes: 60_000, hours: HOUR_MS, days: DAY_MS }[input.unit];
		wake = new Date(now.getTime() + amount * unitMs);
	} else {
		if (!/^\d{4}-\d{2}-\d{2}$/.test(input.date) || !/^\d{2}:\d{2}$/.test(input.time))
			return null;
		wake = new Date(`${input.date}T${input.time}:00`);
		// Reject rolled-over dates and nonexistent local times during DST changes.
		if (localSnoozeDate(wake) !== input.date || localSnoozeTime(wake) !== input.time)
			return null;
	}
	return Number.isFinite(wake.getTime()) && wake.getTime() > now.getTime()
		? wake.getTime()
		: null;
};

/** A local date as YYYY-MM-DD, for a date field. */
export const localSnoozeDate = (date: Date): string =>
	`${String(date.getFullYear()).padStart(4, "0")}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;

/** A local time as HH:MM, for a time field. */
export const localSnoozeTime = (date: Date): string =>
	`${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;

/**
 * Orders editor rows the way the menu lists them: soonest wake first, with
 * options that have no wake right now (a passed "Today") last. Ties keep
 * their current order.
 */
export const sortSnoozeRows = <K>(
	presets: readonly SnoozePresetSetting[],
	keys: readonly K[],
	now: Date
): { readonly presets: readonly SnoozePresetSetting[]; readonly keys: readonly K[] } => {
	const order = presets
		.map((preset, index) => ({
			index,
			wake: resolveSnoozePreset(preset, now)?.until ?? null,
		}))
		.sort((a, b) =>
			a.wake === b.wake ? 0 : a.wake === null ? 1 : b.wake === null ? -1 : a.wake - b.wake
		);
	return {
		presets: order.map(({ index }) => presets[index]!),
		keys: order.map(({ index }) => keys[index]!),
	};
};

/** The units an "after" option's editor counts in. */
export type SnoozeUnit = "minutes" | "hours" | "days";

/** How many minutes each unit holds. */
export const SNOOZE_UNIT_MINUTES: Record<SnoozeUnit, number> = {
	minutes: 1,
	hours: 60,
	days: 24 * 60,
};

/** A duration in the largest unit that divides it evenly, such as 3 hours. */
export const splitSnoozeMinutes = (minutes: number): { amount: number; unit: SnoozeUnit } => {
	if (minutes % SNOOZE_UNIT_MINUTES.days === 0) {
		return { amount: minutes / SNOOZE_UNIT_MINUTES.days, unit: "days" };
	}
	if (minutes % SNOOZE_UNIT_MINUTES.hours === 0) {
		return { amount: minutes / SNOOZE_UNIT_MINUTES.hours, unit: "hours" };
	}
	return { amount: minutes, unit: "minutes" };
};

// Offered in order when adding, skipping any already in the list.
const ADD_CANDIDATES: readonly SnoozePresetSetting[] = [
	...DEFAULT_SNOOZE_PRESETS,
	{ kind: "after", minutes: 30 },
	{ kind: "at", day: "saturday", time: "09:00" },
	{ kind: "after", minutes: 24 * 60 },
];

/** The option "Add option" adds: the first default or extra the list lacks. */
export const nextSnoozeCandidate = (
	presets: readonly SnoozePresetSetting[]
): SnoozePresetSetting => {
	const used = new Set(presets.map(snoozePresetId));
	return (
		ADD_CANDIDATES.find((candidate) => !used.has(snoozePresetId(candidate))) ?? {
			kind: "after",
			minutes: 60,
		}
	);
};

/** Whether two lists hold the same options. Row order is only how they show. */
export const sameSnoozePresets = (
	a: readonly SnoozePresetSetting[],
	b: readonly SnoozePresetSetting[]
): boolean => {
	const ids = (presets: readonly SnoozePresetSetting[]) => presets.map(snoozePresetId).sort();
	const left = ids(a);
	const right = ids(b);
	return left.length === right.length && left.every((id, i) => id === right[i]);
};

/** What an option's "When" can say: a time from now, or a day. */
export type SnoozeWhen = "after" | SnoozePresetDay;

/** The value an option shows in its "When" picker. */
export const snoozeWhenOf = (preset: SnoozePresetSetting): SnoozeWhen =>
	preset.kind === "after" ? "after" : preset.day;

/** The other kind's last values, so a slip on "When" can be undone. */
export type SnoozeKindMemory = { readonly minutes: number; readonly time: string };

/** The option with a new "When", keeping what it can of the old one. */
export const withSnoozeWhen = (
	preset: SnoozePresetSetting,
	when: SnoozeWhen,
	memory: SnoozeKindMemory
): SnoozePresetSetting => {
	if (when === "after") {
		return preset.kind === "after" ? preset : { kind: "after", minutes: memory.minutes };
	}
	return { kind: "at", day: when, time: preset.kind === "at" ? preset.time : memory.time };
};

/** What the editor says beside a row: the menu row it makes, or why it makes none. */
export type SnoozeRowNote =
	| { readonly shown: true; readonly label: string; readonly when: string }
	| { readonly shown: false; readonly text: string };

/** The note beside each row, in the rows' order. */
export const snoozeRowNotes = (
	presets: readonly SnoozePresetSetting[],
	now: Date,
	formatTime?: (date: Date) => string
): SnoozeRowNote[] => {
	const ids = new Set<string>();
	// By minute, as the menu merges them.
	const wakes = new Set<number>();
	return presets.map((preset) => {
		const resolved = resolveSnoozePreset(preset, now, formatTime);
		if (resolved === null) return { shown: false, text: t("Passed for today") };
		if (ids.has(resolved.id)) return { shown: false, text: t("Duplicate of an option above") };
		const minute = Math.floor(resolved.until / 60_000);
		if (wakes.has(minute)) return { shown: false, text: t("Same time as an option above") };
		ids.add(resolved.id);
		wakes.add(minute);
		return { shown: true, label: resolved.label, when: resolved.whenLabel };
	});
};

/** The line under the editor naming what the menu offers now. */
export const snoozeMenuLine = (
	presets: readonly SnoozePresetSetting[],
	now: Date,
	formatTime?: (date: Date) => string
): string => {
	const menu = resolveSnoozePresets(now, { presets, ...(formatTime ? { formatTime } : {}) });
	if (menu.length === 0) return t("Menus offer only Custom…");
	const labels = menu.map((preset) => preset.label);
	const repeated = new Set(labels.filter((label, i) => labels.indexOf(label) !== i));
	const named = menu.map((preset) =>
		repeated.has(preset.label) ? `${preset.label} ${preset.whenLabel}` : preset.label
	);
	return t("Menu now: {{named_join}}", { named_join: named.join(" · ") });
};
