// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Settings that pick a time in minutes, with 0 for never: the time in
// words and the times to pick from.

import { t } from "../i18n";
import type { Option } from "../new-agent/spec";
import { plural } from "../status/format";

/** The minutes in words, such as "15 minutes" or "2 hours", or "Never" for 0. */
export const minutesText = (minutes: number): string => {
	if (minutes === 0) return t("Never");
	const hours = minutes / 60;
	return Number.isInteger(hours) ? plural(hours, "hour") : plural(minutes, "minute");
};

/**
 * The times to pick from, shortest first, with Never first or last as
 * `offered` has it. One set elsewhere that isn't among them shows too, so
 * the pick never reads as nothing.
 */
export const minuteOptions = (offered: readonly number[], current: number): Option[] => {
	const never = offered[0] === 0 ? -1 : Infinity;
	const rank = (minutes: number) => (minutes === 0 ? never : minutes);
	return (offered.includes(current) ? [...offered] : [...offered, current])
		.sort((a, b) => rank(a) - rank(b))
		.map((minutes) => ({ value: String(minutes), label: minutesText(minutes) }));
};
