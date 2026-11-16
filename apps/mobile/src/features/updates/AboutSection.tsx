// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Settings' About card: this build's version, the newest release's iPhone
// build and when the phone last looked, the switch that also looks when the
// app opens, and Check now. A build made on the Mac says it doesn't look.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";
import { useStore } from "zustand";

import { useNow } from "@maxhq/shared/lib/clock";

import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { Toggle } from "../../ui/Toggle";
import { checkNow, keepAutoCheck, openTestflight, thisBuild, updates } from "./store";
import { isNewer, newestNote } from "./updates";

export const AboutSection = () => {
	const { t } = useI18n();

	const state = useStore(updates);
	const now = useNow(30_000);
	const { version, build, repo } = thisBuild;
	const newest = state.newest?.version;
	const newer = Boolean(newest && version && isNewer(newest, version));
	return (
		<Card title={t("About")}>
			<CardRow label={t("Version")}>
				<Text style={styles.value} numberOfLines={1}>
					{build ? `${version ?? "?"} (${build})` : (version ?? "?")}
				</Text>
			</CardRow>
			<CardRow label={t("Newest")} note={newestNote(state, repo, now)}>
				{newest ? (
					<Text style={styles.value} numberOfLines={1}>
						{newest}
					</Text>
				) : null}
				{newer ? (
					<Button
						label={t("Update in TestFlight")}
						tone="link"
						small
						onPress={() => void openTestflight()}
					/>
				) : null}
			</CardRow>
			<CardRow label={t("Check for updates")} note={t("Also when the app opens")}>
				<Toggle
					label={t("Also when the app opens")}
					value={state.autoCheck}
					onChange={keepAutoCheck}
				/>
			</CardRow>
			<View style={styles.actions}>
				<Button
					label={t("Check now")}
					small
					disabled={!repo || state.checking}
					onPress={() => void checkNow()}
				/>
			</View>
		</Card>
	);
};

const styles = StyleSheet.create({
	value: { color: color.secondaryForeground, fontSize: size.small },
	actions: {
		flexDirection: "row",
		paddingHorizontal: 14,
		paddingVertical: 10,
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
	},
});
