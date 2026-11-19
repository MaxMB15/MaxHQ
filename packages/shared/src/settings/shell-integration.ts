// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether a device's terminals give zsh, bash and fish hooks that mark each
// prompt and command, so MaxHQ can tell commands apart. It's on unless you
// turn it off in the device's settings, and off starts shells as their own
// startup files say. A `maxd` from before this setting doesn't report it,
// and adds no hooks.

import type { DeviceId } from "../bindings";
import type { ViewState } from "../view/replica";

/** Whether a device's `maxd` has the setting, as its settings show. */
export const knowsShellIntegration = (view: ViewState, device: DeviceId): boolean =>
	view.tables.device_settings.get(device)?.shell_integration !== undefined;
