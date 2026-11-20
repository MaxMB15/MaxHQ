// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How much one message can carry to an agent whose files upload. A file
// copies to the agent's device before the agent can read it, so one that
// would take the message over a limit fails before it starts. The device
// refuses past the same limits, and only it can count the files inside a
// folder.

import { t } from "../i18n";

const MIB = 1024 * 1024;

/** One attachment this large gets a warning, since it takes a while to copy. */
export const WARN_BYTES = 100 * MIB;

/** The most one message can carry, counting every file in it. */
export const MAX_BYTES = 2 * 1024 * MIB;

/** The most files and folders one message can carry. */
export const MAX_ENTRIES = 20_000;

/** What a file says when it would take the message over the size limit. */
export const TOO_LARGE = "Over the 2 GB a message can carry";

/** What a file says when it would take the message over the entry limit. */
export const TOO_MANY = "Over the 20,000 files a message can carry";

/** What the limits need to know of a file in a message. */
export type Counted = {
	readonly key: string;
	/** The bytes it adds, as far as anyone knows yet. */
	readonly bytes: number;
	/** Whether it goes with the message. A failed file doesn't. */
	readonly counts: boolean;
};

/**
 * Why the file with this key can't go with the ones before it, or null when
 * it fits. A folder counts as one entry here.
 */
export const overLimit = (files: Iterable<Counted>, key: string): string | null => {
	let bytes = 0;
	let entries = 0;
	for (const file of files) {
		if (!file.counts) continue;
		bytes += file.bytes;
		entries += 1;
		if (file.key !== key) continue;
		if (bytes > MAX_BYTES) return t(TOO_LARGE);
		if (entries > MAX_ENTRIES) return t(TOO_MANY);
		return null;
	}
	return null;
};
