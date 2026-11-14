// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { ItemBody, TimelineItem } from "@maxhq/shared/bindings";

/** A timeline item of one kind, with the fields every item has. */
export type ItemOf<K extends ItemBody["kind"]> = Extract<TimelineItem, { kind: K }>;
