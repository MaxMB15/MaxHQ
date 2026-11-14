// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether an agent's `maxd` takes a visual pill. One that doesn't still
// gets the page, as a plain file.

import { type DeviceId, VISUALS_SINCE } from "../bindings";
import type { ViewState } from "./replica";
import { linkForDevice } from "./select";

/**
 * Whether a visual goes as a visual to an agent on `device`. An older
 * `maxd`, or one whose version isn't known yet, gets the page as a file.
 */
export const sendsVisuals = (view: ViewState, device: DeviceId): boolean => {
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	return protocol !== null && protocol >= VISUALS_SINCE;
};
