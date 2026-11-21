// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The five permission modes as the app shows them, and the mode a device
// starts new agents in, as decision C-10 sets it. Codex has no plan mode,
// so Plan is offered to Claude and Cursor. A local ACP agent never shows Auto,
// since MaxHQ knows nothing of its sandbox, and shows Plan only when the
// agent said it has a plan mode.

import { t } from "../i18n";

import type { AcpFeatures, Agent, DeviceId, PermissionMode, Vendor } from "../bindings";
import type { ViewState } from "../view/replica";

/** The modes in menu order, from the most careful to the least. */
export const MODES: readonly PermissionMode[] = [
	"ask",
	"accept_edits",
	"auto",
	"plan",
	"full_access",
];

/** Each mode's name. */
export const MODE_LABEL: Record<PermissionMode, string> = {
	get ask() {
		return t("Ask first");
	},
	get accept_edits() {
		return t("Accept edits");
	},
	get auto() {
		return t("Auto");
	},
	get plan() {
		return t("Plan only");
	},
	get full_access() {
		return t("Full access");
	},
};

/** A few words on what each mode lets the agent do. */
export const MODE_HINT: Record<PermissionMode, string> = {
	get ask() {
		return t("Asks before every edit and command");
	},
	get accept_edits() {
		return t("Edits apply, commands still ask");
	},
	get auto() {
		return t(
			"Approves what looks safe and runs the repo's own test, build and lint scripts without asking. Use it on repos you trust"
		);
	},
	get plan() {
		return t("Reads and plans, changes nothing");
	},
	get full_access() {
		return t("Never asks. Use it in a workspace container");
	},
};

/**
 * Whether the mode shows at all for the vendor. `acp` is what a local ACP
 * agent said it can do, and only counts for one.
 */
export const modeShown = (
	vendor: Vendor,
	mode: PermissionMode,
	acp?: AcpFeatures | null
): boolean => vendor !== "acp" || (mode !== "auto" && (mode !== "plan" || acp?.plan === true));

/** Whether the vendor's CLI can run in the mode. */
export const vendorHasMode = (
	vendor: Vendor,
	mode: PermissionMode,
	acp?: AcpFeatures | null
): boolean => modeShown(vendor, mode, acp) && !(vendor === "codex" && mode === "plan");

/** The mode a device uses before anyone changes it: Auto on a Mac, Full access elsewhere. */
export const osDefault = (os: string | undefined): PermissionMode =>
	os === "macos" ? "auto" : "full_access";

/** The mode the device starts new agents in, as its `maxd` reports it. */
export const deviceDefault = (view: ViewState, device: DeviceId): PermissionMode =>
	view.tables.device_settings.get(device)?.default_permission ??
	osDefault(view.tables.device.get(device)?.os);

/**
 * The mode a new agent on the device starts in. A Codex agent on a device
 * that defaults to Plan asks first instead. A local ACP agent takes Accept
 * edits for Auto, which is what Auto would mean for it.
 */
export const startingMode = (
	view: ViewState,
	device: DeviceId,
	vendor: Vendor,
	acp?: AcpFeatures | null
): PermissionMode => {
	const mode = deviceDefault(view, device);
	if (vendorHasMode(vendor, mode, acp)) return mode;
	return vendor === "acp" && mode === "auto" ? "accept_edits" : "ask";
};

/**
 * The mode an agent runs in: the one it asked for, or else the mode its
 * `device`, when known, starts the vendor's agents in.
 */
export const agentMode = (view: ViewState, agent: Agent, device: DeviceId | null): PermissionMode =>
	agent.permission?.requested ??
	(device === null ? "ask" : startingMode(view, device, agent.vendor));

/** How far each mode lets an agent go, from Plan, which changes nothing. */
const REACH: Record<PermissionMode, number> = {
	plan: 0,
	ask: 1,
	accept_edits: 2,
	auto: 3,
	full_access: 4,
};

/** The narrower of two modes. */
export const narrower = (one: PermissionMode, other: PermissionMode): PermissionMode =>
	REACH[one] <= REACH[other] ? one : other;

/** The mode, or asking first when the vendor lacks it, as its `maxd` does. */
export const runnable = (vendor: Vendor, mode: PermissionMode): PermissionMode =>
	vendorHasMode(vendor, mode) ? mode : "ask";

// The lower and the higher of `ran` and the mode `device` starts the
// vendor's agents in.
const moveBounds = (
	view: ViewState,
	device: DeviceId,
	vendor: Vendor,
	ran: PermissionMode
): [PermissionMode, PermissionMode] => {
	const there = startingMode(view, device, vendor);
	const lower = narrower(ran, there);
	return [lower, lower === ran ? there : ran];
};

/**
 * The modes an agent that ran in `ran` may start in when it moves to
 * `device` under `vendor`, lower first. That device's maxd decides: it
 * takes the lower of `ran` and its own default, and the higher only when
 * you pick it. One mode when they're the same.
 */
export const movedModes = (
	view: ViewState,
	device: DeviceId,
	vendor: Vendor,
	ran: PermissionMode
): PermissionMode[] => {
	const [low, high] = moveBounds(view, device, vendor, ran);
	const [lower, higher] = [runnable(vendor, low), runnable(vendor, high)];
	return lower === higher ? [lower] : [lower, higher];
};

/**
 * The mode a moved agent starts in on `device`: the lower of `ran` and
 * that device's default, or `chosen` up to the higher of the two.
 */
export const movedMode = (
	view: ViewState,
	device: DeviceId,
	vendor: Vendor,
	ran: PermissionMode,
	chosen?: PermissionMode | null
): PermissionMode => {
	const [lower, higher] = moveBounds(view, device, vendor, ran);
	return runnable(vendor, chosen ? narrower(chosen, higher) : lower);
};
