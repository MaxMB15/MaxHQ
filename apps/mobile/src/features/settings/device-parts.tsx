// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a device's settings cards share: the save, which sends all of the
// device's settings with one part changed, as the desktop does, and keeps a
// note for the part that changed; and the rows the cards are built from, a
// picker, a switch and a number field, each with its line under it.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { DeviceId } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import type { Option } from "@maxhq/shared/new-agent/spec";
import {
	type DeviceFields,
	deviceSettingsOf,
	sameDeviceFields,
	type SettingsWhy,
} from "@maxhq/shared/settings/device-settings";
import type { ViewState } from "@maxhq/shared/view/replica";

import { useServices } from "../../services";
import { color, size } from "../../theme";
import { CardRow } from "../../ui/Card";
import { Input } from "../../ui/Field";
import { CardBody, type Note, NoteLine, type SectionProps } from "../devices/section";
import { Picker } from "../tasks/Picker";
import { GatedToggle } from "../test-hosts/parts";
import { draftOf, type Pending, shownFields, type Typed } from "./device-settings";

/** The part of a device's settings a note belongs to. */
export type Part =
	| "mode"
	| "effort"
	| "inbox"
	| "limits"
	| "placement"
	| "idle"
	| "memory"
	| "history"
	| "workspace"
	| "cursor"
	| "awake"
	| "library"
	| "network"
	| "tokens";

export type DeviceSave = {
	/** The settings as the page shows them, with what you just picked. */
	readonly shown: DeviceFields;
	readonly save: (part: Part, change: Partial<DeviceFields>) => Promise<void>;
	readonly noteFor: (part: Part) => Note | null;
	readonly setNote: (part: Part, note: Note) => void;
};

/** Saves `device`'s settings one part at a time. */
export const useDeviceSave = (view: ViewState, device: DeviceId): DeviceSave => {
	const { t } = useI18n();
	const { source } = useServices();
	const current = deviceSettingsOf(view, device);
	const [pending, setPending] = useState<Pending | null>(null);
	const [noted, setNoted] = useState<{ part: Part; note: Note } | null>(null);
	const shown = shownFields(pending, current);
	const setNote = (part: Part, note: Note) => setNoted({ part, note });
	const save = async (part: Part, change: Partial<DeviceFields>) => {
		const fields = { ...shown, ...change };
		if (sameDeviceFields(fields, shown)) return;
		setPending({ fields, from: current });
		setNote(part, { text: t("Saving"), error: false });
		try {
			const failure = failureOf(await source.setDeviceSettings({ device, ...fields }));
			if (failure) setPending(null);
			setNote(
				part,
				failure ? { text: failure, error: true } : { text: t("Saved"), error: false }
			);
		} catch {
			setPending(null);
			setNote(part, { text: t("Couldn't save it. Try again."), error: true });
		}
	};
	const noteFor = (part: Part) => (noted?.part === part ? noted.note : null);
	return { shown, save, noteFor, setNote };
};

/** What each card of a device's settings takes. */
export type CardProps = SectionProps & {
	/** Whether the Mac lets this phone change the device's settings. */
	readonly lets: boolean;
	readonly why: SettingsWhy;
	readonly saver: DeviceSave;
};

/** Whether a row stays off: the reason it can't change, or the Mac's switch. */
export const isOff = (why: string | null, lets: boolean): boolean => why !== null || !lets;

/** A picker with its line under it, and a warning or a note when there is one. */
export const PickRow = ({
	label,
	sub,
	warn,
	value,
	options,
	off,
	note,
	onPick,
}: {
	label: string;
	sub: string;
	warn?: string | null;
	value: string;
	options: readonly Option[];
	off: boolean;
	note?: Note | null;
	onPick: (value: string) => void;
}) => (
	<CardBody>
		<Picker label={label} value={value} options={options} onChange={onPick} disabled={off} />
		<Text style={styles.sub}>{sub}</Text>
		{warn ? <Text style={styles.warn}>{warn}</Text> : null}
		<NoteLine note={note ?? null} />
	</CardBody>
);

/** A switch with its label and line. */
export const SwitchRow = ({
	label,
	sub,
	value,
	off,
	onChange,
}: {
	label: string;
	sub: string;
	value: boolean;
	off: boolean;
	onChange: (on: boolean) => void;
}) => (
	<CardRow label={label} note={sub}>
		<GatedToggle label={label} value={value} disabled={off} onChange={onChange} />
	</CardRow>
);

/**
 * A number field. What you type stays here until you leave the field, so
 * a half-typed number never reaches the device.
 */
export const NumberRow = ({
	label,
	sub,
	value,
	unit,
	off,
	onCommit,
}: {
	label: string;
	sub: string;
	value: number;
	/** What follows the field, such as "days". */
	unit?: string;
	off: boolean;
	/** Takes what you typed, once you're done with it. */
	onCommit: (text: string) => void;
}) => {
	const [typed, setTyped] = useState<Typed | null>(null);
	const draft = draftOf(typed, value);
	const commit = () => {
		setTyped(null);
		if (draft !== null) onCommit(draft);
	};
	return (
		<CardRow label={label} note={sub}>
			<View style={styles.number}>
				<Input
					accessibilityLabel={label}
					accessibilityState={{ disabled: off }}
					value={draft ?? String(value)}
					onChangeText={(text) => setTyped({ text, from: value })}
					onEndEditing={commit}
					keyboardType="number-pad"
					returnKeyType="done"
					selectTextOnFocus
					editable={!off}
					style={[styles.field, off && styles.dim]}
				/>
				{unit ? <Text style={styles.unit}>{unit}</Text> : null}
			</View>
		</CardRow>
	);
};

/** The note of a card's last change, under its rows. */
export const PartNote = ({ note }: { note: Note | null }) =>
	note ? (
		<CardBody>
			<NoteLine note={note} />
		</CardBody>
	) : null;

const styles = StyleSheet.create({
	sub: { color: color.mutedForeground, fontSize: size.meta },
	warn: { color: color.busyText, fontSize: size.meta },
	number: { flexDirection: "row", alignItems: "center", gap: 6 },
	field: { width: 72, textAlign: "right", fontVariant: ["tabular-nums"] },
	unit: { color: color.mutedForeground, fontSize: size.small },
	dim: { opacity: 0.5 },
});
