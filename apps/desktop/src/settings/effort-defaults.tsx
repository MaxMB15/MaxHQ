// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The effort level new Claude Code and Codex agents on a device start at,
// in its New agents card, on a slider over the levels its logins list.
// Until one is picked, each agent starts at the level its model runs at
// when told none, and the slider shows the default model's. A vendor shows
// only once a login of it there lists levels, and a `maxd` from before
// effort levels can't keep one.

import type { DeviceId } from "@maxhq/shared/bindings";
import { useI18n } from "@maxhq/shared/i18n";
import { settingsWhy } from "@maxhq/shared/settings/device-settings";
import { effortDefaultRows, type EffortDefaults } from "@maxhq/shared/settings/effort-defaults";
import type { ViewState } from "@maxhq/shared/view/replica";
import { StepSlider } from "@/components/step-slider";

import { Row, StatusNote } from "./controls";
import type { Note } from "./result";

export type { EffortDefaults };

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
	const why = settingsWhy(view, device, name, live).effort;
	return effortDefaultRows(view, device, shown).map(
		({ vendor, field, title, sub, choices, level }, at) => (
			<Row key={vendor} title={title} sub={why ?? sub}>
				{at === 0 && <StatusNote note={note} />}
				<StepSlider
					label={title}
					stops={choices}
					value={level ?? ""}
					onPick={(value) => onSave({ [field]: value })}
					disabled={why !== null}
				/>
			</Row>
		)
	);
};
