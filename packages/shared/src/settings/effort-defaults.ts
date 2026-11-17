// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The effort level new Claude Code and Codex agents on a device start at,
// in its New agents card. Until one is picked, each agent starts at the
// level its model runs at when told none, and the row shows the default
// model's. A vendor shows only once a login of it there lists levels.

import { deviceEffortChoices, deviceEffortLevel, type EffortChoice } from "../agent-view/effort";
import type { DeviceId, Vendor } from "../bindings";
import { t } from "../i18n";
import type { ViewState } from "../view/replica";

/** The device's default effort level for each vendor, null for each model's own. */
export type EffortDefaults = {
	readonly claude_effort: string | null;
	readonly codex_effort: string | null;
};

/** One vendor's row: what it's called, what it does, and its levels. */
export type EffortDefaultRow = {
	readonly vendor: Vendor;
	readonly field: keyof EffortDefaults;
	readonly title: string;
	readonly sub: string;
	readonly choices: readonly EffortChoice[];
	/** The level new agents start at now, null when no login there lists levels. */
	readonly level: string | null;
};

const VENDORS: readonly { vendor: Vendor; field: keyof EffortDefaults }[] = [
	{ vendor: "claude", field: "claude_effort" },
	{ vendor: "codex", field: "codex_effort" },
];

/** What a vendor's row is called. */
export const effortDefaultTitle = (vendor: Vendor): string =>
	vendor === "claude" ? t("Claude Code effort") : t("Codex effort");

/** The rows for the vendors whose logins on `device` list levels, Claude Code first. */
export const effortDefaultRows = (
	view: ViewState,
	device: DeviceId,
	shown: EffortDefaults
): EffortDefaultRow[] =>
	VENDORS.map(({ vendor, field }) => ({
		vendor,
		field,
		title: effortDefaultTitle(vendor),
		sub:
			vendor === "claude"
				? t(
						"New Claude Code agents here start at this effort level. Change it for one agent from its composer."
					)
				: t("The same for new Codex agents."),
		choices: deviceEffortChoices(view, device, vendor, shown[field]),
		level: deviceEffortLevel(view, device, vendor, shown[field]),
	})).filter((row) => row.choices.length > 0);
