// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the phone's skills screens work out beyond the shared skills logic:
// which device a project's Skills screen shows, and which devices must take
// this phone's gated requests before it may answer a proposal.

import type { DeviceId, ProposalAction, SkillLibrary, SkillProposal } from "@maxhq/shared/bindings";

/**
 * The device a project's Skills screen shows: the one you picked while it
 * still keeps the project's skills, else the device that holds the
 * library when it keeps them, else the first, or null for none.
 */
export const shownDevice = (
	devices: readonly DeviceId[],
	library: SkillLibrary | undefined,
	picked: DeviceId | null
): DeviceId | null => {
	if (picked !== null && devices.includes(picked)) return picked;
	if (library && devices.includes(library.device)) return library.device;
	return devices[0] ?? null;
};

/**
 * The device besides the proposal's own that an approve also reaches: the
 * one that holds the library, when that's another device, or null.
 */
export const approveHolder = (
	library: SkillLibrary | undefined,
	proposal: SkillProposal
): DeviceId | null => (library && library.device !== proposal.device ? library.device : null);

/**
 * Whether this phone may send `action` for a proposal. A reject goes to
 * the proposal's device only; an approve also reaches the device that
 * holds the library, so both must take this phone's gated requests.
 */
export const mayAnswer = (
	action: ProposalAction,
	holder: DeviceId | null,
	letsHere: boolean,
	letsHolder: boolean
): boolean => letsHere && (action === "reject" || holder === null || letsHolder);
