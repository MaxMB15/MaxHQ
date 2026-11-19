// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A workspace being made, as every client follows it: the steps the device
// named so far with a dot each, how many are left, and whether it's done.
// The device names only the step it's on, so a client keeps the ones it
// has seen by their place.

import { t } from "../i18n";

import type { Workspace, WorkspaceState } from "../bindings";
import type { ConnectionLook } from "../status/connection";

/** A step's dot and its word. */
export type StepLook = { readonly look: ConnectionLook; readonly label: string };

const DONE: StepLook = {
	look: "live",
	get label() {
		return t("Done");
	},
};
const RUNNING: StepLook = {
	look: "connecting",
	get label() {
		return t("Running");
	},
};
const FAILED: StepLook = {
	look: "unreachable",
	get label() {
		return t("Failed");
	},
};
const UNKNOWN: StepLook = {
	look: "unknown",
	get label() {
		return t("Unknown");
	},
};

/** The name a workspace being made goes by: the device's, once it gives one. */
export const madeInstance = (workspace: Workspace | undefined, planned: string): string =>
	workspace?.location.type === "container" ? workspace.location.instance : planned;

/** Whether the device has finished making the workspace, one way or the other. */
export const madeOrFailed = (workspace: Workspace | undefined): boolean => {
	const state = workspace?.managed?.state;
	return !state || state.state === "running" || state.state === "failed";
};

/** The steps seen so far, by their place. */
export type SeenSteps = ReadonlyMap<number, string>;

/**
 * The steps seen with the one `state` is on added, or `seen` itself when
 * it already has it, so a client sets its state only on a change.
 */
export const seeStep = (seen: SeenSteps, state: WorkspaceState | undefined): SeenSteps =>
	state?.state === "creating" && seen.get(state.done) !== state.step
		? new Map(seen).set(state.done, state.step)
		: seen;

/** One step on the list: its place, its words and its dot. */
export type CreationStep = { readonly at: number; readonly text: string } & StepLook;

/** The steps seen in order, each with its dot as `state` has it. */
export const creationSteps = (
	seen: SeenSteps,
	state: WorkspaceState | undefined
): CreationStep[] => {
	const steps = [...seen].sort(([a], [b]) => a - b);
	const last = steps[steps.length - 1]?.[0];
	const lookOf = (i: number): StepLook => {
		switch (state?.state) {
			case "creating":
				return i < state.done ? DONE : RUNNING;
			case "running":
				return DONE;
			case "failed":
				return i === last ? FAILED : DONE;
			default:
				return UNKNOWN;
		}
	};
	return steps.map(([at, text]) => ({ at, text, ...lookOf(at) }));
};

/** How many steps come after the one it's on, while it's being made. */
export const stepsLeft = (state: WorkspaceState | undefined): number =>
	state?.state === "creating" ? Math.max(0, state.of - state.done - 1) : 0;
