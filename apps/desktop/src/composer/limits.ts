// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How much one message can carry to a remote agent, counted over the
// composer's chips. The limits themselves are shared with the phone.

import type { Chip } from "./attachments";
import { overLimit } from "@maxhq/shared/composer/limits";

export {
	MAX_BYTES,
	MAX_ENTRIES,
	TOO_LARGE,
	TOO_MANY,
	WARN_BYTES,
} from "@maxhq/shared/composer/limits";

/** The bytes a chip adds to its message, as far as anyone knows yet. */
export const sizeOf = (chip: Chip): number => Math.max(chip.bytes ?? 0, chip.upload?.total ?? 0);

/**
 * Why the chip with this key can't go with the ones before it, or null when
 * it fits. Failed chips don't count, since they never go. A folder counts
 * as one entry here.
 */
export const refusalFor = (chips: readonly Chip[], key: string): string | null =>
	overLimit(
		chips.map((chip) => ({
			key: chip.key,
			bytes: sizeOf(chip),
			counts: chip.state !== "failed",
		})),
		key
	);
