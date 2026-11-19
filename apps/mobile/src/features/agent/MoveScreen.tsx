// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Move sheet, from the agent's More menu, as the desktop's Move dialog:
// the device to move the agent to, the account there, the model, and the
// folder there, a checkout of the same repository picked first. It says
// what carries over and what stays, and while a turn runs its button reads
// Stop and move. The Mac that runs MaxHQ moves the agent, which needs its
// switch for this phone, so the button stays in view, off, until the Mac
// turns it on. Once the new agent starts, its screen takes the sheet's place.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { Platform, ScrollView, StyleSheet, Text, View } from "react-native";

import { failureOf } from "@maxhq/shared/agent-view/answer";
import {
	agentDevice,
	MOVE_FOLDER,
	type MoveChoices,
	moveForm,
	NO_MOVE_CHOICES,
} from "@maxhq/shared/agent-view/move-agent";
import { dispatcherDevice } from "@maxhq/shared/dispatch/reach";
import { useNow } from "@maxhq/shared/lib/clock";
import { deviceName } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Header } from "../../ui/Header";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { Hint, Part, TextField } from "../tasks/fields";
import { Picker } from "../tasks/Picker";

export const MoveScreen = ({ navigation, route }: ScreenProps<"MoveAgent">) => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const now = useNow(30_000);
	const agent = view.tables.agent.get(route.params.agent);
	const [choices, setChoices] = useState<MoveChoices>(NO_MOVE_CHOICES);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const home = dispatcherDevice(view);
	const allowed = useLetsPhone(home);

	const close = () => navigation.goBack();
	if (!agent) {
		return (
			<View style={styles.screen}>
				<Header
					title={t("Move to…")}
					sheet={Platform.OS === "ios"}
					back={{ label: t("Close"), onPress: close, close: true }}
				/>
				<Text style={styles.gone}>{t("This agent is gone.")}</Text>
			</View>
		);
	}

	const form = moveForm(view, agent, choices, now);
	const pick = (patch: Partial<MoveChoices>) => {
		setChoices((c) => ({ ...c, ...patch }));
		setError(null);
	};
	const couldNot = (e: unknown) =>
		e instanceof Error && e.message
			? t("Couldn't {{what}}. {{message}}", { what: t("move the agent"), message: e.message })
			: t("Couldn't {{what}}.", { what: t("move the agent") });

	const move = () => {
		const from = agentDevice(view, agent);
		if (!form.to || home === null || from === null || busy || !allowed) return;
		setBusy(true);
		setError(null);
		services.source.moveAgent(home, agent.id, from, form.to, form.stop).then(
			(result) => {
				setBusy(false);
				const why = failureOf(result);
				if (why) return setError(why);
				if (result.status === "ok" && result.data.status === "started") {
					navigation.replace("Agent", { agent: result.data.agent });
				} else close();
			},
			(e: unknown) => {
				setBusy(false);
				setError(couldNot(e));
			}
		);
	};

	const there = form.device === null ? "" : deviceName(view, form.device);
	const status = error ?? form.problem;
	return (
		<View style={styles.screen}>
			<Header
				title={t("Move {{title}} to another device", {
					title: agent.title || t("This agent"),
				})}
				sheet={Platform.OS === "ios"}
				back={{ label: t("Cancel"), onPress: close, close: true }}
			/>
			<ScrollView
				contentContainerStyle={styles.content}
				keyboardShouldPersistTaps="handled"
				keyboardDismissMode="interactive"
				automaticallyAdjustKeyboardInsets
			>
				<Hint>
					{t("A new agent takes over on the device you pick, under the same name.")}
				</Hint>
				<Part label={t("Move to…")}>
					<Picker
						label={t("Device")}
						value={form.device ?? ""}
						options={form.devices}
						onChange={(device) => pick({ device, account: null, place: null })}
						disabled={busy}
					/>
					<Picker
						label={t("Account")}
						value={form.account?.id ?? ""}
						options={form.accounts}
						onChange={(account) => pick({ account, model: null })}
						disabled={busy || form.device === null}
					/>
					{form.missing.length > 0 && form.device !== null ? (
						<Hint>{t("Not signed in on {{names}}", { names: there })}</Hint>
					) : null}
					<Picker
						label={t("Model")}
						value={form.model}
						options={form.models}
						onChange={(model) => pick({ model })}
						disabled={busy || !form.account}
					/>
					<Picker
						label={t("Folder")}
						value={form.place ?? ""}
						options={form.places}
						onChange={(place) => pick({ place })}
						disabled={busy || form.device === null}
					/>
					{form.place === MOVE_FOLDER ? (
						<TextField
							label={t("Folder")}
							value={choices.path}
							onChange={(path) => pick({ path })}
							placeholder={t("/Users/you/code/project")}
							mono
						/>
					) : null}
				</Part>
				<View style={styles.lines}>
					{form.lines.map((line) => (
						<View key={line} style={styles.line}>
							<Text style={styles.bullet}>•</Text>
							<Text style={styles.lineText}>{line}</Text>
						</View>
					))}
				</View>
				{status ? (
					<Text
						style={[styles.status, error !== null && styles.failed]}
						accessibilityRole={error !== null ? "alert" : undefined}
						accessibilityLiveRegion="polite"
					>
						{status}
					</Text>
				) : null}
				<Button
					label={form.action}
					tone="primary"
					disabled={busy || form.to === null || !allowed}
					onPress={move}
				/>
				{home !== null && !allowed ? <NeedsSwitch /> : null}
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	gone: { color: color.mutedForeground, fontSize: size.body, padding: 16 },
	lines: { gap: 6 },
	line: { flexDirection: "row", gap: 8 },
	bullet: { color: color.mutedForeground, fontSize: size.small, lineHeight: 19 },
	lineText: { flex: 1, color: color.secondaryForeground, fontSize: size.small, lineHeight: 19 },
	status: { color: color.mutedForeground, fontSize: size.small },
	failed: { color: color.errorText },
});
