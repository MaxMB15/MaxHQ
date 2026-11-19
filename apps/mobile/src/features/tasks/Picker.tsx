// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A field that picks one of a list, as the desktop's picker chips do: its
// label, and a box with what's picked that opens the list in a sheet. Each
// entry has its second line and its tag, and one that can't be picked shows
// dimmed. The task editor picks its trigger, target, home and new agent
// with it.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useMemo, useState } from "react";
import { FlatList, Modal, Pressable, StyleSheet, Text, View } from "react-native";

import type { Option } from "@maxhq/shared/new-agent/spec";

import { color, radius, size, TOUCH } from "../../theme";
import { Check } from "../../ui/glyphs";
import { Header } from "../../ui/Header";
import { Chevron } from "../../ui/Icon";
import { Label } from "./fields";
import { pickedLabel, pickerRows } from "./pickers";

const Entry = memo(
	({
		option,
		picked,
		onPick,
	}: {
		option: Option;
		picked: boolean;
		onPick: (value: string) => void;
	}) => (
		<Pressable
			accessibilityRole="button"
			accessibilityState={{ selected: picked, disabled: Boolean(option.disabled) }}
			disabled={option.disabled}
			onPress={() => onPick(option.value)}
			style={({ pressed }) => [
				styles.entry,
				picked && styles.picked,
				pressed && styles.pressed,
				option.disabled && styles.off,
			]}
		>
			<View style={styles.text}>
				<Text style={styles.entryLabel} numberOfLines={1}>
					{option.label}
				</Text>
				{option.detail ? (
					<Text style={[styles.detail, option.warn && styles.warn]} numberOfLines={2}>
						{option.detail}
					</Text>
				) : null}
			</View>
			{option.tag ? <Text style={styles.tag}>{option.tag}</Text> : null}
			{picked ? <Check size={15} strokeWidth={2.4} color={color.primary} /> : null}
		</Pressable>
	)
);
Entry.displayName = "PickerEntry";

/** Picks one of `options`. */
export const Picker = ({
	label,
	value,
	options,
	onChange,
	disabled,
	empty,
}: {
	label: string;
	value: string;
	options: readonly Option[];
	onChange: (value: string) => void;
	disabled?: boolean;
	/** What shows while nothing is picked. */
	empty?: string;
}) => {
	const { t } = useI18n();
	const [open, setOpen] = useState(false);
	const rows = useMemo(() => pickerRows(options), [options]);
	const off = disabled || options.length === 0;
	const shown = pickedLabel(options, value, empty ?? t("None"));
	const pick = (next: string) => {
		setOpen(false);
		if (next !== value) onChange(next);
	};
	return (
		<View style={styles.field}>
			<Label>{label}</Label>
			<Pressable
				accessibilityRole="button"
				accessibilityLabel={label}
				accessibilityValue={{ text: shown }}
				accessibilityState={{ disabled: off }}
				disabled={off}
				onPress={() => setOpen(true)}
				style={({ pressed }) => [styles.box, pressed && styles.pressed, off && styles.off]}
			>
				<Text style={styles.value} numberOfLines={1}>
					{shown}
				</Text>
				<Chevron dir="down" size={12} />
			</Pressable>
			<Modal
				visible={open}
				animationType="slide"
				presentationStyle="pageSheet"
				onRequestClose={() => setOpen(false)}
			>
				<View style={styles.sheet}>
					<Header
						title={label}
						sheet
						back={{ label: t("Close"), onPress: () => setOpen(false), close: true }}
					/>
					<FlatList
						data={rows}
						keyExtractor={(row) => row.key}
						contentContainerStyle={styles.list}
						renderItem={({ item }) =>
							item.kind === "group" ? (
								<Text style={styles.group} accessibilityRole="header">
									{item.label}
								</Text>
							) : (
								<Entry
									option={item.option}
									picked={item.option.value === value}
									onPick={pick}
								/>
							)
						}
					/>
				</View>
			</Modal>
		</View>
	);
};

const styles = StyleSheet.create({
	field: { gap: 6 },
	box: {
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.subpanel,
		paddingHorizontal: 12,
	},
	value: { flex: 1, color: color.foreground, fontSize: size.body },
	sheet: { flex: 1, backgroundColor: color.background },
	list: { padding: 12, paddingBottom: 40 },
	group: {
		color: color.mutedForeground,
		fontSize: size.tiny,
		fontWeight: "600",
		paddingHorizontal: 4,
		paddingTop: 14,
		paddingBottom: 6,
	},
	entry: {
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		borderRadius: radius.md,
		paddingHorizontal: 12,
		paddingVertical: 10,
	},
	picked: { backgroundColor: color.secondary },
	pressed: { backgroundColor: color.accent },
	off: { opacity: 0.45 },
	text: { flex: 1, gap: 2 },
	entryLabel: { color: color.foreground, fontSize: size.body },
	detail: { color: color.mutedForeground, fontSize: size.meta },
	warn: { color: color.busyText },
	tag: { color: color.mutedForeground, fontSize: size.tiny },
});
