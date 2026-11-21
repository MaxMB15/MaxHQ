// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Short forms for times, durations and token counts. Every function takes
// `now` from the caller, so a render stays pure and tests stay exact. Clock
// times use the local time zone.

import { t, formatDate, formatNumber, formatList } from "../i18n";

import type { Timestamp } from "../bindings";

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** A duration in words, such as "12 s", "1 min 12 s", "2 h 22 min" or "3 d 4 h". */
export const formatDuration = (ms: number): string => {
	const total = Math.max(0, Math.round(ms / SECOND));
	const days = Math.floor(total / 86400);
	const hours = Math.floor((total % 86400) / 3600);
	const minutes = Math.floor((total % 3600) / 60);
	const seconds = total % 60;
	if (days > 0)
		return hours > 0
			? t("{{days, number}} d {{hours, number}} h", { days, hours })
			: t("{{count, number}} d", { count: days });
	if (hours > 0)
		return minutes > 0
			? t("{{hours, number}} h {{minutes, number}} min", { hours, minutes })
			: t("{{count, number}} h", { count: hours });
	if (minutes > 0) {
		return seconds > 0 && minutes < 10
			? t("{{minutes, number}} min {{seconds, number}} s", { minutes, seconds })
			: t("{{count, number}} min", { count: minutes });
	}
	return t("{{count, number}} s", { count: seconds });
};

/** How long ago, in words, such as "2 s ago" or "4 min ago". */
export const formatAgo = (at: Timestamp, now: Timestamp): string => {
	const ms = now - at;
	if (ms < 2 * SECOND) return t("just now");
	if (ms >= 10 * MINUTE)
		return t("{{duration}} ago", {
			duration: formatDuration(Math.floor(ms / MINUTE) * MINUTE),
		});
	return t("{{duration}} ago", { duration: formatDuration(ms) });
};

/** How long ago, as a sidebar row shows it, such as "now", "4m", "3h" or "2d". */
export const formatAgoShort = (at: Timestamp, now: Timestamp): string => {
	const ms = Math.max(0, now - at);
	return ms < MINUTE ? t("now") : formatSpanShort(ms);
};

/** A stretch of time in its largest whole unit, such as "4m", "3h" or "2d", and never under "1m". */
export const formatSpanShort = (ms: number): string => {
	if (ms < HOUR) return t("{{count, number}}m", { count: Math.max(1, Math.floor(ms / MINUTE)) });
	if (ms < DAY) return t("{{count, number}}h", { count: Math.floor(ms / HOUR) });
	return t("{{count, number}}d", { count: Math.floor(ms / DAY) });
};

/** The local clock time, such as "17:10". */
export const formatClock = (at: Timestamp): string => {
	return formatDate(at, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
};

const sameDay = (a: Timestamp, b: Timestamp) => {
	const x = new Date(a);
	const y = new Date(b);
	return (
		x.getFullYear() === y.getFullYear() &&
		x.getMonth() === y.getMonth() &&
		x.getDate() === y.getDate()
	);
};

/**
 * The clock time alone when it's today, or with the weekday when it's
 * within a week, such as "Thu 09:00". Further out it adds the date.
 */
export const formatWhen = (at: Timestamp, now: Timestamp): string => {
	if (sameDay(at, now)) return formatClock(at);
	if (Math.abs(at - now) < 6 * DAY)
		return t("{{day}} {{time}}", {
			day: formatDate(at, { weekday: "short" }),
			time: formatClock(at),
		});
	return t("{{date}} {{time}}", {
		date: formatDate(at, { day: "numeric", month: "numeric" }),
		time: formatClock(at),
	});
};

/** The day and time in full, such as "Thursday, November 26 at 02:00", for a tooltip. */
export const formatFullWhen = (at: Timestamp): string =>
	formatDate(at, {
		weekday: "long",
		day: "numeric",
		month: "long",
		hour: "2-digit",
		minute: "2-digit",
		hourCycle: "h23",
	});

/** US cents as dollars, such as "$12.40" or "$50". */
export const formatDollars = (cents: number): string =>
	formatNumber(cents / 100, {
		style: "currency",
		currency: "USD",
		minimumFractionDigits: cents % 100 === 0 ? 0 : 2,
		maximumFractionDigits: 2,
	});

/** A token count, such as "118k" or "1M". */
export const formatTokens = (tokens: number): string => {
	if (tokens >= 1_000_000) {
		const millions = tokens / 1_000_000;
		return t("{{value}}M", { value: formatNumber(millions, { maximumFractionDigits: 1 }) });
	}
	if (tokens >= 1000) return t("{{value}}k", { value: formatNumber(Math.round(tokens / 1000)) });
	return formatNumber(tokens);
};

/** How full a context window is, such as "118k of 200k". */
export const formatContext = (used: number, window: number): string =>
	t("{{used}} of {{total}}", { used: formatTokens(used), total: formatTokens(window) });

/** A percentage rounded for display, such as "62%". */
export const formatPercent = (percent: number): string =>
	formatNumber(percent / 100, { style: "percent", maximumFractionDigits: 0 });

/** A count with its noun, such as "1 agent" or "4 agents". */
export const plural = (count: number, one: string, many = `${one}s`): string =>
	t(`{{count, number}} ${one}`, {
		count,
		defaultValue: `${formatNumber(count)} ${count === 1 ? one : many}`,
	});

/** A list in words, such as "a, b and c". */
export const listInWords = (items: readonly string[]): string => {
	return formatList(items);
};

// Names that stay in lower case at the start of a sentence.
const LOWER_NAMES = new Set(["maxd", "ssh"]);

// Words whose 's means "is", where it's a name's after any other word.
const PRONOUNS = new Set(["it", "there", "that", "what", "here", "who"]);

/**
 * A reason a backend sends, as the start of a sentence: the first letter in
 * capitals unless the first word is a name such as a host, and no closing
 * period, since callers add their own.
 */
export const sentence = (text: string): string => {
	const trimmed = text.trim().replace(/\.+$/, "");
	const first = /^\S+/.exec(trimmed)?.[0] ?? "";
	// A word such as "couldn't" or "it's" counts, but "devbox's" is a name's.
	const [, stem, ending] = /^([a-z]+)(?:['’]([a-z]+))?,?$/.exec(first) ?? [];
	const plainWord =
		stem !== undefined && (ending === "s" ? PRONOUNS.has(stem) : !LOWER_NAMES.has(stem));
	return plainWord ? trimmed[0]!.toUpperCase() + trimmed.slice(1) : trimmed;
};

/**
 * Whether `maxd` cut a summary short, which it marks with an ellipsis,
 * sometimes inside the backticks or before the question mark that ends
 * it, as in "Run `cargo test…`?".
 */
export const isClipped = (summary: string): boolean => /…`?\??$/.test(summary.trim());

// Characters that can change what text seems to say without showing anything:
// control characters other than a line break or a tab, soft hyphens, zero-width
// and direction marks, and tag characters.
const HIDDEN =
	// eslint-disable-next-line no-control-regex
	/\r(?!\n)|[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\u00AD\u061C\u180E\u200B-\u200F\u2028-\u202E\u2060-\u206F\uFEFF\uFFF9-\uFFFB\u{E0000}-\u{E007F}]/gu;

/**
 * Text with what it would hide spelled out, as in "[U+202E]", so a request you
 * answer reads the way it will run.
 */
export const showHidden = (text: string): string =>
	text.replace(
		HIDDEN,
		(char) => `[U+${(char.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, "0")}]`
	);

// The title maxd gives a tool request whose agent named no tool, in the
// one language maxd writes.
const ASKS_FOR_A_TOOL = /^(.+) asks to use a tool$/;

/**
 * A tool request's title in the reader's language when maxd wrote it, as it
 * does when the agent named no tool. A title from the agent stays as it is.
 */
export const askTitle = (title: string): string => {
	const named = ASKS_FOR_A_TOOL.exec(title);
	return named ? t("{{name}} asks to use a tool", { name: named[1] }) : title;
};

/** Spells small counts as words, such as "Second" for a second attempt. */
export const ordinal = (n: number): string =>
	[t("First"), t("Second"), t("Third"), t("Fourth"), t("Fifth")][n - 1] ??
	t("Attempt {{n}}", { n: n });
