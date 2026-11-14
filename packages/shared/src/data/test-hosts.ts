// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the test host calls answer before test hosts are built, the way
// `maxd` refuses them.

import type { ActionResult } from "./source";

/** Why a test host call was refused before test hosts are built. */
export const TEST_HOSTS_NOT_SET_UP = "Test hosts aren't set up yet.";

/** What a test host call gets before test hosts are built. */
export const testHostsNotSetUp = async (): Promise<ActionResult> => ({
	status: "ok",
	data: { status: "refused", reason: TEST_HOSTS_NOT_SET_UP },
});
