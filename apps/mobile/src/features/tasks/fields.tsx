// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The task editor's building blocks, as the desktop's: a card for each
// part, a label over a field, a text field, a line of help under it, a
// switch with its words, a row of chips that each turn one thing on, and
// a tinted box for something to act on now, such as a URL that shows once.

import type { ReactNode } from "react";
import { Pressable, StyleSheet, Text, type TextInputProps, View } from "react-native";

import type { TaskDraft } from "@maxhq/shared/tasks/draft";

import { color, font, radius, size, tint } from "../../theme";
import { Card } from "../../ui/Card";
import { Input } from "../../ui/Field";
import { Toggle } from "../../ui/Toggle";

/** A change to some of the draft's fields. */
export type Edit = (patch: Partial<TaskDraft>) => void;

/** The small label over a field. */
export const Label = ({ children }: { children: string }) => (
	<Text style={styles.label}>{children}</Text>
);

/** A card with a title over a group of fields, or with `label` for VoiceOver when it has none. */
export const Part = ({
	title,
	label,
	children,
}: {
	title?: string;
	label?: string;
	children: ReactNode;
}) => (
	<Card title={title} label={label}>
		<View style={[styles.part, !title && styles.untitled]}>{children}</View>
	</Card>
);

/** A line of help under a field, amber for a warning. */
export const Hint = ({ children, warn }: { children: ReactNode; warn?: boolean }) => (
	<Text style={[styles.hint, warn && styles.warn]}>{children}</Text>
);

/** A labelled text field. */
export const TextField = ({
	label,
	value,
	onChange,
	placeholder,
	mono,
	secure,
	lines,
	keyboardType,
}: {
	label: string;
	value: string;
	onChange: (value: string) => void;
	placeholder?: string;
	/** Code-like text, such as a header or a key: monospaced, never corrected. */
	mono?: boolean;
	/** A secret: hidden, never corrected or saved by the keyboard. */
	secure?: boolean;
	/** How many lines it shows, for a field that takes several. */
	lines?: number;
	keyboardType?: TextInputProps["keyboardType"];
}) => {
	const plain = mono || secure || keyboardType !== undefined;
	return (
		<View style={styles.field}>
			<Label>{label}</Label>
			<Input
				accessibilityLabel={label}
				value={value}
				onChangeText={onChange}
				placeholder={placeholder}
				secureTextEntry={secure}
				textContentType={secure ? "password" : undefined}
				autoComplete="off"
				autoCapitalize={plain ? "none" : undefined}
				autoCorrect={plain ? false : undefined}
				spellCheck={plain ? false : undefined}
				keyboardType={keyboardType}
				multiline={lines !== undefined}
				numberOfLines={lines}
				textAlignVertical={lines !== undefined ? "top" : undefined}
				style={[
					mono && styles.mono,
					lines !== undefined && { minHeight: 22 + lines * 19, lineHeight: 19 },
				]}
			/>
		</View>
	);
};

/** A switch with the words for what it turns on, and a note under them. */
export const Tick = ({
	label,
	note,
	value,
	onChange,
}: {
	label: string;
	note?: string;
	value: boolean;
	onChange: (on: boolean) => void;
}) => (
	<View style={styles.tick}>
		<View style={styles.tickText}>
			<Text style={styles.tickLabel}>{label}</Text>
			{note ? <Text style={styles.hint}>{note}</Text> : null}
		</View>
		<Toggle label={label} value={value} onChange={onChange} />
	</View>
);

/** A chip in a row of them that each turn one thing on, such as a weekday. */
export type Chip = { readonly key: string; readonly label: string; readonly on: boolean };

/** A row of chips, wrapping; a tap turns one on or off. */
export const Chips = ({
	label,
	chips,
	onToggle,
}: {
	label: string;
	chips: readonly Chip[];
	onToggle: (key: string, on: boolean) => void;
}) => (
	<View style={styles.field}>
		<Label>{label}</Label>
		<View accessibilityLabel={label} style={styles.chips}>
			{chips.map((chip) => (
				<Pressable
					key={chip.key}
					accessibilityRole="checkbox"
					accessibilityLabel={chip.label}
					accessibilityState={{ checked: chip.on }}
					onPress={() => onToggle(chip.key, !chip.on)}
					style={({ pressed }) => [
						styles.chip,
						chip.on && styles.chipOn,
						pressed && !chip.on && styles.pressed,
					]}
				>
					<Text style={[styles.chipText, chip.on && styles.chipTextOn]}>
						{chip.label}
					</Text>
				</Pressable>
			))}
		</View>
	</View>
);

/** A tinted box: blue for something to take now, amber for something to fix. */
export const Notice = ({
	title,
	warn,
	children,
}: {
	title: string;
	warn?: boolean;
	children: ReactNode;
}) => (
	<View
		accessibilityRole={warn ? "alert" : undefined}
		style={[styles.notice, warn ? styles.noticeWarn : styles.noticeInfo]}
	>
		<Text style={styles.noticeTitle}>{title}</Text>
		{children}
	</View>
);

/** Code-like text in a field's place, such as a URL or a command. */
export const Mono = ({ children, lines = 1 }: { children: string; lines?: number }) => (
	<Text selectable numberOfLines={lines} style={styles.code}>
		{children}
	</Text>
);

const styles = StyleSheet.create({
	part: { gap: 14, paddingHorizontal: 14, paddingBottom: 14 },
	untitled: { paddingTop: 14 },
	field: { gap: 6 },
	label: { color: color.secondaryForeground, fontSize: size.meta, fontWeight: "500" },
	hint: { color: color.mutedForeground, fontSize: size.meta, lineHeight: 17 },
	warn: { color: color.busyText },
	mono: { fontFamily: font.mono, fontSize: size.meta },
	tick: { flexDirection: "row", alignItems: "center", gap: 12 },
	tickText: { flex: 1, gap: 2 },
	tickLabel: { color: color.foreground, fontSize: size.small },
	chips: { flexDirection: "row", flexWrap: "wrap", gap: 6 },
	chip: {
		minWidth: 44,
		minHeight: 36,
		alignItems: "center",
		justifyContent: "center",
		paddingHorizontal: 10,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
	},
	chipOn: { borderColor: color.primary, backgroundColor: tint(color.primary, 0.15) },
	pressed: { backgroundColor: color.accent },
	chipText: { color: color.mutedForeground, fontSize: size.meta },
	chipTextOn: { color: color.foreground },
	notice: { gap: 8, padding: 12, borderRadius: radius.md, borderWidth: 1 },
	noticeInfo: {
		borderColor: tint(color.primary, 0.4),
		backgroundColor: tint(color.primary, 0.08),
	},
	noticeWarn: { borderColor: tint(color.busy, 0.4), backgroundColor: tint(color.busy, 0.08) },
	noticeTitle: { color: color.foreground, fontSize: size.meta, fontWeight: "600" },
	code: {
		flex: 1,
		minWidth: 0,
		color: color.foreground,
		fontFamily: font.mono,
		fontSize: size.meta,
	},
});
