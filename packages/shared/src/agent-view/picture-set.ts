// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Going through a set of pictures shown whole: the ones a message attached,
// a step showed the agent or a reply shows. A set stops at its ends rather
// than wrapping, so the first and the last are plain to see.

/** The picture `delta` steps from `index` in a set of `count`, kept within the set. */
export const stepPicture = (index: number, count: number, delta: number): number =>
	Math.min(Math.max(index + delta, 0), Math.max(count - 1, 0));

/** The pictures either side of `index`, to fetch ahead of a step. */
export const pictureNeighbours = (index: number, count: number): readonly number[] =>
	[index - 1, index + 1].filter((i) => i >= 0 && i < count);
