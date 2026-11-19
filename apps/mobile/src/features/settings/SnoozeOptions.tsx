// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The snooze menu's options on this phone, as the desktop's editor: each
// option is a time from now or a time on a day, with the menu row it makes
// beside it. Rows sort soonest first when the page opens, and stay put
// while you edit, so a row never moves under your finger. Each change
// saves as you make it.

import { formatNumber, t, useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { Pressable, StyleSheet, Text, TextInput, View } from "react-native";

import { useNow } from "@maxhq/shared/lib/clock";
import { formatClock } from "@maxhq/shared/status/format";
import {
	DEFAULT_SNOOZE_PRESETS,
	MAX_SNOOZE_PRESET_MINUTES,
	MAX_SNOOZE_PRESETS,
	nextSnoozeCandidate,
	sameSnoozePresets,
	SNOOZE_PRESET_DAYS,
	SNOOZE_UNIT_MINUTES,
	type SnoozeKindMemory,
	snoozeMenuLine,
	snoozePresetId,
	type SnoozePresetSetting,
	snoozeRowNotes,
	type SnoozeUnit,
	type SnoozeWhen,
	snoozeWhenOf,
	sortSnoozeRows,
	splitSnoozeMinutes,
	withSnoozeWhen,
} from "@maxhq/shared/view/snooze";

import { color, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { choose } from "../../ui/choose";
import { IconButton } from "../../ui/Header";
import { Chevron, Close } from "../../ui/Icon";
import { CardBody } from "../devices/section";
import { keepInboxPrefs, useInboxPrefs } from "../inbox/prefs";

const WHEN_LABELS: Record<SnoozeWhen, string> = {
	after: "In",
	today: "Today",
	tomorrow: "Tomorrow",
	monday: "Monday",
	tuesday: "Tuesday",
	wednesday: "Wednesday",
	thursday: "Thursday",
	friday: "Friday",
	saturday: "Saturday",
	sunday: "Sunday",
};
const WHENS: readonly SnoozeWhen[] = ["after", ...SNOOZE_PRESET_DAYS];
const UNITS: readonly SnoozeUnit[] = ["minutes", "hours", "days"];
const ONE_UNIT: Record<SnoozeUnit, string> = { minutes: "minute", hours: "hour", days: "day" };
const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;

const formatTime = (date: Date): string => formatClock(date.getTime());

/** A small box that shows its pick and opens the choices in a sheet. */
const Pick = <T extends string>({
	label,
	value,
	options,
	onChange,
}: {
	label: string;
	value: T;
	options: readonly { readonly value: T; readonly label: string }[];
	onChange: (value: T) => void;
}) => {
	const shown = options.find((option) => option.value === value)?.label ?? "";
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={label}
			accessibilityValue={{ text: shown }}
			onPress={() =>
				choose(
					label,
					options.map((option) => ({
						label: option.label,
						disabled: option.value === value,
						pick: () => onChange(option.value),
					}))
				)
			}
			style={({ pressed }) => [styles.box, pressed && styles.pressed]}
		>
			<Text style={styles.value} numberOfLines={1}>
				{shown}
			</Text>
			<Chevron dir="down" size={10} />
		</Pressable>
	);
};

/**
 * A short text box. What you type stays here until you leave it, then
 * `commit` takes it, or it goes back when `commit` turns it down.
 */
const Short = ({
	label,
	value,
	keyboard,
	commit,
}: {
	label: string;
	value: string;
	keyboard: "number-pad" | "numbers-and-punctuation";
	commit: (text: string) => void;
}) => {
	const [typed, setTyped] = useState<{ text: string; from: string } | null>(null);
	const draft = typed && typed.from === value ? typed.text : null;
	return (
		<TextInput
			accessibilityLabel={label}
			value={draft ?? value}
			onChangeText={(text) => setTyped({ text, from: value })}
			onEndEditing={() => {
				setTyped(null);
				if (draft !== null) commit(draft.trim());
			}}
			keyboardType={keyboard}
			returnKeyType="done"
			selectTextOnFocus
			autoCorrect={false}
			spellCheck={false}
			style={[styles.box, styles.short]}
		/>
	);
};

const Duration = ({
	minutes,
	onChange,
}: {
	minutes: number;
	onChange: (minutes: number) => void;
}) => {
	// The chosen unit stays put, so 59 to 60 minutes doesn't jump to hours.
	const [chosen, setChosen] = useState(() => splitSnoozeMinutes(minutes).unit);
	const unit =
		minutes % SNOOZE_UNIT_MINUTES[chosen] === 0 ? chosen : splitSnoozeMinutes(minutes).unit;
	const amount = minutes / SNOOZE_UNIT_MINUTES[unit];
	const most = Math.floor(MAX_SNOOZE_PRESET_MINUTES / SNOOZE_UNIT_MINUTES[unit]);
	const names = amount === 1 ? ONE_UNIT : null;
	return (
		<>
			<Short
				label={t("Amount")}
				value={String(amount)}
				keyboard="number-pad"
				commit={(text) => {
					// Anything but a whole number puts the amount back.
					if (!/^\d+$/.test(text)) return;
					const next = Math.min(most, Math.max(1, Number(text)));
					if (next !== amount) onChange(next * SNOOZE_UNIT_MINUTES[unit]);
				}}
			/>
			<Pick
				label={t("Unit")}
				value={unit}
				options={UNITS.map((value) => ({ value, label: t(names?.[value] ?? value) }))}
				onChange={(next) => {
					setChosen(next);
					onChange(
						Math.min(amount * SNOOZE_UNIT_MINUTES[next], MAX_SNOOZE_PRESET_MINUTES)
					);
				}}
			/>
		</>
	);
};

const Fields = ({
	preset,
	onChange,
}: {
	preset: SnoozePresetSetting;
	onChange: (preset: SnoozePresetSetting) => void;
}) => {
	// Switching between a duration and a set time keeps the other kind's last
	// value, so a slip on "When" can be undone without retyping it.
	const [memory, setMemory] = useState<SnoozeKindMemory>({ minutes: 60, time: "09:00" });
	return (
		<View style={styles.fields}>
			<Pick
				label={t("When")}
				value={snoozeWhenOf(preset)}
				options={WHENS.map((value) => ({ value, label: t(WHEN_LABELS[value]) }))}
				onChange={(when) => {
					const next = withSnoozeWhen(preset, when, memory);
					if (next.kind !== preset.kind) {
						setMemory(
							preset.kind === "after"
								? { ...memory, minutes: preset.minutes }
								: { ...memory, time: preset.time }
						);
					}
					onChange(next);
				}}
			/>
			{preset.kind === "after" ? (
				<Duration
					minutes={preset.minutes}
					onChange={(minutes) => onChange({ ...preset, minutes })}
				/>
			) : (
				<>
					<Text style={styles.note}>{t("at")}</Text>
					<Short
						label={t("Time")}
						value={preset.time}
						keyboard="numbers-and-punctuation"
						commit={(text) => {
							if (TIME.test(text) && text !== preset.time)
								onChange({ ...preset, time: text });
						}}
					/>
				</>
			)}
		</View>
	);
};

/** The Snooze options card. */
export const SnoozeOptions = () => {
	useI18n();
	const saved = useInboxPrefs((prefs) => prefs.snoozePresets);
	const now = new Date(useNow(60_000));
	// Row keys survive removals, so a row's half-typed field stays with it.
	const [rows, setRows] = useState(() =>
		sortSnoozeRows(
			saved,
			saved.map((_, i) => i),
			now
		)
	);
	const { presets, keys } = rows;
	const [nextKey, setNextKey] = useState(saved.length);
	const freshKeys = (count: number) => {
		setNextKey(nextKey + count);
		return Array.from({ length: count }, (_, i) => nextKey + i);
	};
	const save = (next: readonly SnoozePresetSetting[], nextKeys: readonly number[] = keys) => {
		setRows({ presets: next, keys: nextKeys });
		keepInboxPrefs({ snoozePresets: next });
	};
	const replaceAt = (index: number, next: SnoozePresetSetting) => {
		if (snoozePresetId(next) === snoozePresetId(presets[index]!)) return;
		save(presets.map((preset, i) => (i === index ? next : preset)));
	};

	const notes = snoozeRowNotes(presets, now, formatTime);
	const full = presets.length >= MAX_SNOOZE_PRESETS;
	return (
		<Card
			title={t("Snooze options")}
			note={t(
				"Choices in every snooze menu, listed soonest first. Weekdays mean the next one after today."
			)}
		>
			{presets.map((preset, i) => {
				const note = notes[i]!;
				return (
					<CardBody key={keys[i]}>
						<View
							style={styles.row}
							accessibilityLabel={t("Snooze option {{value1}}", { value1: i + 1 })}
						>
							<Fields preset={preset} onChange={(next) => replaceAt(i, next)} />
							<IconButton
								label={t("Remove snooze option {{value1}}", { value1: i + 1 })}
								onPress={() =>
									save(
										presets.filter((_, at) => at !== i),
										keys.filter((_, at) => at !== i)
									)
								}
							>
								<Close size={14} />
							</IconButton>
						</View>
						{note.shown ? (
							<Text style={styles.note} numberOfLines={1}>
								<Text style={styles.label}>{note.label}</Text>
								{`  ${note.when}`}
							</Text>
						) : (
							<Text style={styles.note}>{note.text}</Text>
						)}
					</CardBody>
				);
			})}
			<CardBody>
				<Text style={styles.note}>{snoozeMenuLine(presets, now, formatTime)}</Text>
				<View style={styles.buttons}>
					<Button
						label={t("Add option")}
						small
						disabled={full}
						onPress={() =>
							save(
								[...presets, nextSnoozeCandidate(presets)],
								[...keys, ...freshKeys(1)]
							)
						}
					/>
					{sameSnoozePresets(presets, DEFAULT_SNOOZE_PRESETS) ? null : (
						<Button
							label={t("Reset to defaults")}
							small
							onPress={() => {
								const sorted = sortSnoozeRows(
									DEFAULT_SNOOZE_PRESETS,
									freshKeys(DEFAULT_SNOOZE_PRESETS.length),
									now
								);
								save(sorted.presets, sorted.keys);
							}}
						/>
					)}
				</View>
				{full ? (
					<Text style={styles.note}>
						{t("Up to ")}
						{formatNumber(MAX_SNOOZE_PRESETS)}
						{t(" options")}
					</Text>
				) : null}
			</CardBody>
		</Card>
	);
};

const styles = StyleSheet.create({
	row: { flexDirection: "row", alignItems: "center", gap: 4 },
	fields: { flex: 1, flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: 8 },
	box: {
		minHeight: 32,
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		paddingHorizontal: 10,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
	},
	pressed: { backgroundColor: color.accent },
	value: { color: color.foreground, fontSize: size.small, fontWeight: "500" },
	short: {
		width: 64,
		color: color.foreground,
		fontSize: size.small,
		textAlign: "right",
		fontVariant: ["tabular-nums"],
	},
	label: { color: color.foreground },
	note: { color: color.mutedForeground, fontSize: size.meta },
	buttons: { flexDirection: "row", flexWrap: "wrap", gap: 8 },
});
