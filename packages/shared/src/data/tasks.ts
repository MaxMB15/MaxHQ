// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the scheduled task calls answer before scheduled tasks are built,
// the way `maxd` refuses them.

import { t } from "../i18n";

import type { CommandError } from "../bindings";

/** What a scheduled task call gets before scheduled tasks are built. */
export const tasksNotBuilt = async (): Promise<{ status: "error"; error: CommandError }> => ({
	status: "error",
	error: { kind: "failed", message: t("Scheduled tasks aren't built yet.") },
});
