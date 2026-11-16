// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The phone's colors for a workspace's state and a move's steps, as the
// desktop draws them, and how full a workspace's bar is while it's made.

import type { StepStatus } from "@maxhq/shared/hosts/moves";
import type { ConnectionLook } from "@maxhq/shared/status/connection";

import { color } from "../../theme";

/** The color of a state's words, matching its dot. */
export const LOOK_TEXT: Readonly<Record<ConnectionLook, string>> = {
	live: color.readyText,
	asleep: color.mutedForeground,
	connecting: color.connectingText,
	agent_down: color.linked,
	unreachable: color.errorText,
	unknown: color.mutedForeground,
};

/** A move step's dot: filled, or a dashed ring for a step not reached yet. */
export type StepDot = { readonly color: string; readonly dashed: boolean };

export const STEP_DOT: Readonly<Record<StepStatus, StepDot>> = {
	done: { color: color.ready, dashed: false },
	ready: { color: color.primary, dashed: false },
	running: { color: color.connecting, dashed: false },
	failed: { color: color.error, dashed: false },
	todo: { color: color.off, dashed: true },
};

/** How much of a workspace's bar is full while it's made, from 0 to 1. */
export const madeShare = (done: number, of: number): number =>
	Math.min(1, Math.max(0, done / Math.max(1, of)));
