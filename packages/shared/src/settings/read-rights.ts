// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether agents on a device may list and read other agents, here and on
// linked devices, without a permission. It's on unless you turn it off in
// the device's settings, and off brings back reads of only what you
// attached or quoted. Starting, messaging, stopping and moving agents still
// need a permission either way. A `maxd` from before this setting doesn't
// report it, and lets agents read only what they're allowed.

import type { DeviceId } from "../bindings";
import type { ViewState } from "../view/replica";

/** Whether a device's `maxd` has the setting, as its settings show. */
export const knowsReadRights = (view: ViewState, device: DeviceId): boolean =>
	view.tables.device_settings.get(device)?.read_other_agents !== undefined;
