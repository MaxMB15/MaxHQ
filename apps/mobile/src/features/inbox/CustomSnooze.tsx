// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The custom snooze dialog, as the desktop has it: a date and time, picked
// with the phone's own pickers, or a duration, for when snoozed agents come
// back. The snooze sheet asks for a
// time with `requestCustomSnooze`, and the agent list draws the dialog.
// Edit snooze options opens the list's settings, where the sheet's choices
// change.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { KeyboardAvoidingView, Modal, StyleSheet, Text, View } from "react-native";
import { createStore, useStore } from "zustand";

import type { Timestamp } from "@maxhq/shared/bindings";
import {
	type CustomSnoozeInput,
	localSnoozeDate,
	localSnoozeTime,
	resolveCustomSnooze,
} from "@maxhq/shared/view/snooze";

import { color, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { DateTimeField } from "../../ui/DateTimeField";
import { Tabs } from "../pulls/Tabs";
import { TextField } from "../tasks/fields";

type Request = { readonly resolve: (until: Timestamp | null) => void };

const requests = createStore<{ readonly request: Request | null }>(() => ({ request: null }));

/**
 * Asks when to wake, in the custom snooze dialog. Resolves with the time,
 * or null when you cancel. A new request cancels the one before.
 */
export const requestCustomSnooze = (): Promise<Timestamp | null> => {
	requests.getState().request?.resolve(null);
	return new Promise((resolve) => requests.setState({ request: { resolve } }));
};

const finish = (until: Timestamp | null) => {
	const request = requests.getState().request;
	requests.setState({ request: null });
	request?.resolve(until);
};

type Mode = CustomSnoozeInput["mode"];
type Unit = "minutes" | "hours" | "days";

const Form = ({ onEditOptions }: { onEditOptions: () => void }) => {
	const { t } = useI18n();
	const [opened] = useState(() => new Date());
	const [mode, setMode] = useState<Mode>("date");
	const [when, setWhen] = useState(() => new Date(opened.getTime() + 3_600_000));
	const [amount, setAmount] = useState("2");
	const [unit, setUnit] = useState<Unit>("hours");
	const [error, setError] = useState<string | null>(null);
	const edit =
		<T,>(set: (value: T) => void) =>
		(value: T) => {
			set(value);
			setError(null);
		};

	const snooze = () => {
		const input: CustomSnoozeInput =
			mode === "date"
				? { mode, date: localSnoozeDate(when), time: localSnoozeTime(when) }
				: { mode, amount, unit };
		const until = resolveCustomSnooze(input, new Date());
		if (until === null) {
			setError(
				mode === "date"
					? t("Choose a valid date and time in the future.")
					: t("Enter a positive duration.")
			);
			return;
		}
		finish(until);
	};

	return (
		<View style={styles.card} accessibilityViewIsModal>
			<View style={styles.head}>
				<Text style={styles.title} accessibilityRole="header">
					{t("Custom snooze")}
				</Text>
				<Text style={styles.note}>
					{t("Choose when snoozed agents return to your inbox.")}
				</Text>
			</View>
			<Tabs
				tabs={[
					{ value: "date", label: t("Date and time") },
					{ value: "duration", label: t("Duration") },
				]}
				value={mode}
				onChange={edit(setMode)}
			/>
			{mode === "date" ? (
				<View style={styles.pair}>
					<View style={styles.half}>
						<DateTimeField
							label={t("Date")}
							mode="date"
							value={when}
							minimumDate={opened}
							onChange={edit(setWhen)}
						/>
					</View>
					<View style={styles.half}>
						<DateTimeField
							label={t("Time")}
							mode="time"
							value={when}
							onChange={edit(setWhen)}
						/>
					</View>
				</View>
			) : (
				<>
					<TextField
						label={t("Snooze for")}
						value={amount}
						onChange={edit(setAmount)}
						keyboardType="decimal-pad"
					/>
					<Tabs
						tabs={[
							{ value: "minutes", label: t("Minutes") },
							{ value: "hours", label: t("Hours") },
							{ value: "days", label: t("Days") },
						]}
						value={unit}
						onChange={edit(setUnit)}
					/>
				</>
			)}
			{error ? (
				<Text accessibilityRole="alert" style={styles.error}>
					{error}
				</Text>
			) : null}
			<View style={styles.buttons}>
				<Button label={t("Cancel")} grow={1} onPress={() => finish(null)} />
				<Button label={t("Snooze")} tone="primary" grow={1} onPress={snooze} />
			</View>
			<Button
				label={t("Edit snooze options")}
				small
				onPress={() => {
					finish(null);
					onEditOptions();
				}}
			/>
		</View>
	);
};

/** The dialog, drawn while a request waits; `onEditOptions` opens the list's settings. */
export const CustomSnoozeHost = ({ onEditOptions }: { onEditOptions: () => void }) => {
	const open = useStore(requests, (s) => s.request !== null);
	return (
		<Modal visible={open} transparent animationType="fade" onRequestClose={() => finish(null)}>
			<KeyboardAvoidingView behavior="padding" style={styles.scrim}>
				{/* A fresh form each time, so it starts an hour from now. */}
				{open ? <Form onEditOptions={onEditOptions} /> : null}
			</KeyboardAvoidingView>
		</Modal>
	);
};

const styles = StyleSheet.create({
	scrim: {
		flex: 1,
		justifyContent: "center",
		padding: 24,
		backgroundColor: "rgba(0, 0, 0, 0.55)",
	},
	card: {
		gap: 14,
		padding: 16,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.card,
	},
	head: { gap: 4 },
	title: { color: color.foregroundStrong, fontSize: size.heading, fontWeight: "600" },
	note: { color: color.mutedForeground, fontSize: size.small },
	pair: { flexDirection: "row", gap: 10 },
	half: { flex: 1 },
	error: { color: color.errorText, fontSize: size.small },
	buttons: { flexDirection: "row", gap: 10 },
});
