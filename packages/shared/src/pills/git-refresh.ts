// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A pull request or issue goes out as it was when you picked it, unless
// that was a while ago. Then send looks it up again, briefly, and keeps the
// old record when the lookup fails or takes too long, so it never holds a
// message back.

import type { Timestamp } from "../bindings";

import type { GitItemRef } from "./draft";

/** How old a pull request or issue gets before send looks it up again. */
export const GIT_STALE_MS = 5 * 60_000;

/** How long send waits for each lookup. */
export const GIT_REFRESH_TIMEOUT_MS = 3000;

/** What `work` gives, or null once `ms` pass first. */
const within = <T>(work: Promise<T>, ms: number): Promise<T | null> =>
	new Promise((resolve, reject) => {
		const timer = setTimeout(() => resolve(null), ms);
		work.then(
			(value) => {
				clearTimeout(timer);
				resolve(value);
			},
			(error: unknown) => {
				clearTimeout(timer);
				reject(error);
			}
		);
	});

/**
 * The items, each checked more than five minutes before `now` swapped for
 * what `find` gives for it now. The lookups run side by side, and one that
 * fails, finds nothing or is slow leaves its item as it was.
 */
export const refreshGitItems = (
	items: readonly GitItemRef[],
	find: (item: GitItemRef) => Promise<GitItemRef | null>,
	now: Timestamp
): Promise<GitItemRef[]> =>
	Promise.all(
		items.map(async (item) => {
			if (now - item.checked_at <= GIT_STALE_MS) return item;
			const fresh = await within(find(item), GIT_REFRESH_TIMEOUT_MS).catch(() => null);
			return fresh ?? item;
		})
	);
