// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How long an agent sits idle before its device's `maxd` stops its CLI to
// free the memory. The agent stays as it is, and its CLI starts again for
// the next message. A `maxd` from before this setting doesn't report it.

import type { DeviceId } from "../bindings";
import type { Option } from "../new-agent/spec";
import type { ViewState } from "../view/replica";

import { minuteOptions } from "./minutes";

/** The idle minutes before a CLI stops, unless a device says otherwise. */
export const DEFAULT_IDLE_RELEASE_MINUTES = 60;

/** The times Settings offers, in minutes, with 0 for never. */
export const IDLE_RELEASE_MINUTES = [0, 15, 30, 60, 120, 240, 480] as const;

/** Whether a device's `maxd` stops idle CLIs, as its settings show. */
export const releasesIdle = (view: ViewState, device: DeviceId): boolean =>
	view.tables.device_settings.get(device)?.idle_release_minutes !== undefined;

/** The times to pick from, Never first. */
export const idleReleaseOptions = (current: number): Option[] =>
	minuteOptions(IDLE_RELEASE_MINUTES, current);
