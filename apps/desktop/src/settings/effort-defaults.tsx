// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The effort level new Claude Code and Codex agents on a device start at,
// in its New agents card, on a slider over the levels its logins list.
// Until one is picked, each agent starts at the level its model runs at
// when told none, and the slider shows the default model's. A vendor shows
// only once a login of it there lists levels, and a `maxd` from before
// effort levels can't keep one.

import {
	deviceEffortChoices,
	deviceEffortLevel,
	setsEffort,
} from "@maxhq/shared/agent-view/effort";
import type { DeviceId, Vendor } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import type { ViewState } from "@maxhq/shared/view/replica";
import { StepSlider } from "@/components/step-slider";

import { Row, StatusNote } from "./controls";
import type { Note } from "./result";

/** The device's default effort level for each vendor, null for each model's own. */
export type EffortDefaults = {
	readonly claude_effort: string | null;
	readonly codex_effort: string | null;
};

const VENDORS: readonly { vendor: Vendor; field: keyof EffortDefaults }[] = [
	{ vendor: "claude", field: "claude_effort" },
	{ vendor: "codex", field: "codex_effort" },
];

const title = (vendor: Vendor): string =>
	vendor === "claude" ? translate("Claude Code effort") : translate("Codex effort");

export const EffortDefaultRows = ({
	view,
	device,
	name,
	live,
	shown,
	note,
	onSave,
}: {
	view: ViewState;
	device: DeviceId;
	/** The device's name, as the sidebar shows it. */
	name: string;
	/** Whether the device's `maxd` answers, so it can take the change. */
	live: boolean;
	shown: EffortDefaults;
	note: Note | null;
	onSave: (change: Partial<EffortDefaults>) => void;
}) => {
	useI18n();
	const why = !live
		? translate("Connect to {{name}} to change it.", { name: name })
		: !setsEffort(view, device)
			? translate("{{name}}'s maxd is too old for this. Update it first.", { name: name })
			: null;
	const rows = VENDORS.map(({ vendor, field }) => ({
		vendor,
		field,
		choices: deviceEffortChoices(view, device, vendor, shown[field]),
		level: deviceEffortLevel(view, device, vendor, shown[field]),
	})).filter((row) => row.choices.length > 0);
	return rows.map(({ vendor, field, choices, level }, at) => (
		<Row
			key={vendor}
			title={title(vendor)}
			sub={
				why ??
				(vendor === "claude"
					? translate(
							"New Claude Code agents here start at this effort level. Change it for one agent from its composer."
						)
					: translate("The same for new Codex agents."))
			}
		>
			{at === 0 && <StatusNote note={note} />}
			<StepSlider
				label={title(vendor)}
				stops={choices}
				value={level ?? ""}
				onPick={(value) => onSave({ [field]: value })}
				disabled={why !== null}
			/>
		</Row>
	));
};
