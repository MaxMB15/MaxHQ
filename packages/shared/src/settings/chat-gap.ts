// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How long after a dispatcher chat's last message your next request starts
// a fresh chat, as the device that runs the dispatcher keeps it. Never keeps
// you in the chat you're in until you start a new conversation or the
// dispatcher moves to another topic. A `maxd` from before this setting
// doesn't report it.

import type { DeviceId } from "../bindings";
import type { Option } from "../new-agent/spec";
import type { ViewState } from "../view/replica";

import { minuteOptions } from "./minutes";

/** The pause before a fresh chat, unless a device says otherwise. */
export const DEFAULT_CHAT_GAP_MINUTES = 30;

/** The pauses Settings offers, in minutes, with 0 for never. */
export const CHAT_GAP_MINUTES = [15, 30, 60, 120, 240, 0] as const;

/**
 * The pause after which `device`'s dispatcher starts a fresh chat, or null
 * when its `maxd` is too old for the setting.
 */
export const chatGap = (view: ViewState, device: DeviceId): number | null =>
	view.tables.device_settings.get(device)?.chat_gap_minutes ?? null;

/** The pauses to pick from, Never last. */
export const chatGapOptions = (current: number): Option[] =>
	minuteOptions(CHAT_GAP_MINUTES, current);
