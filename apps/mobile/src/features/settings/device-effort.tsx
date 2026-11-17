// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The effort level new Claude Code and Codex agents on a device start at,
// in its New agents card. Each row opens the effort dialog an agent's own
// level uses, over the levels the device's logins list. A vendor shows
// only once a login of it there lists levels.

import { useState } from "react";
import { StyleSheet, Text } from "react-native";

import type { DeviceId } from "@maxhq/shared/bindings";
import {
	effortDefaultRows,
	type EffortDefaultRow,
	type EffortDefaults,
} from "@maxhq/shared/settings/effort-defaults";
import type { ViewState } from "@maxhq/shared/view/replica";

import { color, size } from "../../theme";
import { CardRow } from "../../ui/Card";
import { EffortDialog } from "../agent/EffortDialog";
import { effortValue } from "./device-settings";

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
}) => {
	const [open, setOpen] = useState<EffortDefaultRow | null>(null);
	return (
		<>
			{effortDefaultRows(view, device, shown).map((row) => (
				<CardRow
					key={row.vendor}
					label={row.title}
					note={why ?? row.sub}
					onPress={off ? undefined : () => setOpen(row)}
				>
					<Text style={[styles.value, off && styles.dim]}>{effortValue(row)}</Text>
				</CardRow>
			))}
			<EffortDialog
				pick={open && { message: open.title, levels: open.choices, current: open.level }}
				onPick={(level) => {
					if (open) onSave({ [open.field]: level });
				}}
				close={() => setOpen(null)}
			/>
		</>
	);
};

const styles = StyleSheet.create({
	value: { color: color.secondaryForeground, fontSize: size.small },
	dim: { opacity: 0.5 },
});
