// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How the agent header makes room as its pane narrows. Its labels fold to
// their icons one at a time, in a fixed order, so they never run into each
// other or into the title. Each folded label keeps its full text in a
// tooltip and still does what a click on it did.

import { foldsAt } from "../ui/fold";

/** A label the header can fold. */
export type HeaderLabel =
	"openIn" | "context" | "branch" | "device" | "folder" | "account" | "model";

// Below each width in pixels, that label shows only its icon. Open in goes
// first, as its icon says enough, then the context count, the branch, the
// device and the folder, whose name matters more than its device's, then
// the account. The model goes last and whole, as the composer shows it too.
const FOLDS: readonly (readonly [HeaderLabel, number])[] = [
	["openIn", 960],
	["context", 860],
	["branch", 760],
	["device", 660],
	["folder", 580],
	["account", 520],
	["model", 480],
];

// About how wide a chip after the model is, such as "In a terminal, read
// only" or a PR. Each one the header shows takes that much from the labels.
const CHIP_PX = 120;

/**
 * How wide the header is at each level, for `fitLevel`, with this many
 * chips after the model. The header renders again only when its level
 * changes, not on every pixel of a resize.
 */
export const headerNeeds = (chips = 0): number[] => [
	...FOLDS.map(([, below]) => below + chips * CHIP_PX),
	0,
];

/** Which labels are folded at a level. */
export const foldedAt = (level: number): ReadonlySet<HeaderLabel> =>
	foldsAt(
		FOLDS.map(([label]) => label),
		level
	);
