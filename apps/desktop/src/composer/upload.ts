// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sums behind the upload strip: how far a message's uploads have got
// and how fast they go. A remote agent reads its files only once they've
// copied to its device, so the strip says how long that has left.

import { type Chip, formatBytes, type RateSample } from "./attachments";
import { sizeOf, WARN_BYTES } from "./limits";
import { t as translate } from "@maxhq/shared/i18n";

/** What the warning line says about attachments over 100 MB, or null when none is. */
export const largeWarning = (chips: readonly Chip[]): string | null => {
	const large = chips.filter((chip) => chip.state !== "failed" && sizeOf(chip) >= WARN_BYTES);
	const [first] = large;
	if (!first) return null;
	return large.length === 1
		? translate("{{name}} is {{bytes}}, so it takes a while to copy.", {
				name: first.name,
				bytes: formatBytes(sizeOf(first)),
			})
		: translate("{{length}} attachments are over 100 MB, so they take a while to copy.", {
				length: large.length,
			});
};

/** Where a message's uploads stand, for the strip. */
export type UploadTotals = {
	/** The files still on their way. */
	readonly active: number;
	/** Bytes sent so far, across the files still on their way. */
	readonly sent: number;
	/** Bytes in all, across the files still on their way. */
	readonly total: number;
	/** Whole percent, from 0 to 100. */
	readonly percent: number;
	/** Whether any upload waits for the link to come back. */
	readonly paused: boolean;
	/** Whether any upload waits for room on the device. */
	readonly waiting: boolean;
	/** Failed uploads that Retry can try again. */
	readonly retryable: number;
};

/** Sums up the chips that upload. */
export const uploadTotals = (chips: readonly Chip[]): UploadTotals => {
	let active = 0;
	let sent = 0;
	let total = 0;
	let paused = false;
	let waiting = false;
	let retryable = 0;
	for (const chip of chips) {
		if (chip.state === "failed" && chip.retryable && chip.upload) retryable += 1;
		if (chip.state !== "uploading" || !chip.upload) continue;
		active += 1;
		const size = sizeOf(chip);
		sent += Math.min(chip.upload.sent, size);
		total += size;
		paused ||= chip.upload.state === "paused";
		waiting ||= chip.upload.state === "waiting";
	}
	const percent = total > 0 ? Math.min(100, Math.floor((sent / total) * 100)) : 0;
	return { active, sent, total, percent, paused, waiting, retryable };
};

/** Bytes a second over the samples, or null until there's enough to tell. */
export const rateOf = (samples: readonly RateSample[]): number | null => {
	const first = samples[0];
	const last = samples.at(-1);
	if (!first || !last || last.at - first.at < 250) return null;
	const rate = ((last.sent - first.sent) / (last.at - first.at)) * 1000;
	return rate > 0 ? rate : null;
};

/** A speed, such as "1.2 MB/s". */
export const formatRate = (bytesPerSecond: number): string =>
	`${formatBytes(Math.round(bytesPerSecond))}/s`;
