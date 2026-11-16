// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual in a feed or a note, where Markdown and the feed's rows put it:
// its card, which draws the page there, with Open, which shows it whole.

import type { VisualToOpen } from "./visual";
import { VisualCard } from "./VisualCard";

/** A visual's card, with `meta` after its title, such as that it's a new version. */
export const VisualOpenRow = ({ visual, meta }: { visual: VisualToOpen; meta?: string | null }) => (
	<VisualCard visual={visual} meta={meta} />
);
