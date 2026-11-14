// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How much one message can carry to a remote agent. A file copies to the
// agent's device before the agent can read it, so a chip that would take
// the message over a limit fails before it starts. The device refuses past
// the same limits, and only it can count the files inside a folder.

import type { Chip } from "./attachments";
import { t as translate } from "@maxhq/shared/i18n";

const MIB = 1024 * 1024;

/** One attachment this large gets a warning, since it takes a while to copy. */
export const WARN_BYTES = 100 * MIB;

/** The most one message can carry, counting every file in it. */
export const MAX_BYTES = 2 * 1024 * MIB;

/** The most files and folders one message can carry. */
export const MAX_ENTRIES = 20_000;

/** What a chip says when it would take the message over the size limit. */
export const TOO_LARGE = "Over the 2 GB a message can carry";

/** What a chip says when it would take the message over the entry limit. */
export const TOO_MANY = "Over the 20,000 files a message can carry";

/** The bytes a chip adds to its message, as far as anyone knows yet. */
export const sizeOf = (chip: Chip): number => Math.max(chip.bytes ?? 0, chip.upload?.total ?? 0);

/**
 * Why the chip with this key can't go with the ones before it, or null when
 * it fits. Failed chips don't count, since they never go. A folder counts
 * as one entry here.
 */
export const refusalFor = (chips: readonly Chip[], key: string): string | null => {
	let bytes = 0;
	let entries = 0;
	for (const chip of chips) {
		if (chip.state === "failed") continue;
		bytes += sizeOf(chip);
		entries += 1;
		if (chip.key !== key) continue;
		if (bytes > MAX_BYTES) return translate(TOO_LARGE);
		if (entries > MAX_ENTRIES) return translate(TOO_MANY);
		return null;
	}
	return null;
};
