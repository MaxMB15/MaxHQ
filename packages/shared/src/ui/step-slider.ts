// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A slider that snaps to a few named stops, such as effort levels, as the
// desktop and the phone both draw it. Each stop has a column of the same
// width with the stop at its center. A pick shows at once, before the
// device has it; a drag shows where it goes and picks when it lets go.

import { t } from "../i18n";

/** One stop on the slider. */
export type Stop = {
	readonly value: string;
	readonly label: string;
	/** Whether it's the one used when nothing is picked. */
	readonly isDefault?: boolean;
};

/** The stop picked on the slider, and the value the slider had then. */
export type Picked = { readonly from: string; readonly to: string };

/**
 * The value the slider shows: the stop picked on it until `value` moves
 * away from what it was then, so the thumb doesn't jump back while the pick
 * is on its way.
 */
export const shownValue = (picked: Picked | null, value: string): string =>
	picked !== null && picked.from === value ? picked.to : value;

/** Where `value` sits among `stops`, or the first stop when it's none of them. */
export const stopIndex = (stops: readonly Stop[], value: string): number =>
	Math.max(
		0,
		stops.findIndex((stop) => stop.value === value)
	);

/**
 * The stop a pick at `index` lands on, held to the slider's ends, or null
 * when that's the stop it shows already.
 */
export const stopToPick = (stops: readonly Stop[], index: number, shown: string): Stop | null => {
	const stop = stops[Math.min(Math.max(index, 0), stops.length - 1)];
	return !stop || stop.value === shown ? null : stop;
};

/** The stop under a point `x` from the start of a slider `width` wide. */
export const stopAt = (x: number, width: number, count: number): number =>
	Math.floor((Math.min(Math.max(x, 0), width - 1) / width) * count);

/** Where a stop's center sits, as a share of the slider's width. */
export const stopCenter = (index: number, count: number): number => (index + 0.5) / count;

/** What a screen reader says for a stop: its name, and whether it's the default. */
export const stopText = (stop: Stop): string =>
	stop.isDefault ? t("{{level}} (default)", { level: stop.label }) : stop.label;
