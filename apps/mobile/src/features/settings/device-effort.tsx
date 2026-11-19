// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The effort level new Claude Code and Codex agents on a device start at,
// in its New agents card, on a slider over the levels its logins list, as
// the desktop has it. Until one is picked, each agent starts at the level
// its model runs at when told none, and the slider shows the default
// model's. A vendor shows only once a login of it there lists levels.

import { StyleSheet, Text, View } from "react-native";

import type { DeviceId } from "@maxhq/shared/bindings";
import { effortDefaultRows, type EffortDefaults } from "@maxhq/shared/settings/effort-defaults";
import type { ViewState } from "@maxhq/shared/view/replica";

import { color, size } from "../../theme";
import { StepSlider } from "../../ui/StepSlider";
import { CardBody } from "../devices/section";

export const EffortRows = ({
	view,
	device,
	why,
	off,
	shown,
	onSave,
}: {
	view: ViewState;
	device: DeviceId;
	/** Why the levels can't change now, or null. */
	why: string | null;
	off: boolean;
	shown: EffortDefaults;
	onSave: (change: Partial<EffortDefaults>) => void;
}) =>
	effortDefaultRows(view, device, shown).map(({ vendor, field, title, sub, choices, level }) => (
		<CardBody key={vendor}>
			<View style={styles.text}>
				<Text style={styles.title}>{title}</Text>
				<Text style={styles.sub}>{why ?? sub}</Text>
			</View>
			<StepSlider
				label={title}
				stops={choices}
				value={level ?? ""}
				onPick={(value) => onSave({ [field]: value })}
				disabled={off}
			/>
		</CardBody>
	));

const styles = StyleSheet.create({
	text: { gap: 1 },
	title: { color: color.foreground, fontSize: size.small },
	sub: { color: color.mutedForeground, fontSize: size.meta },
});
