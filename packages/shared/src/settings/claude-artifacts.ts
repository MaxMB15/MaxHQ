// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether Claude agents on a device may use Claude Code's tools for
// claude.ai pages: publishing pages to your claude.ai account, reading
// pages you or others share, and changing their data and comments. It's on
// unless you turn it off in the device's settings, and off takes the tools
// away from Claude agents there the next time their CLI starts. A `maxd`
// from before this setting doesn't report it.

import type { DeviceId } from "../bindings";
import type { ViewState } from "../view/replica";

/** Whether a device's `maxd` has the setting, as its settings show. */
export const knowsClaudeArtifacts = (view: ViewState, device: DeviceId): boolean =>
	view.tables.device_settings.get(device)?.claude_artifacts !== undefined;
