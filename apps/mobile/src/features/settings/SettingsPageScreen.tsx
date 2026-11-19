// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A page Settings opens, as the desktop's App group has them: the agent
// list's shelves and snooze options, the composer, notifications, the
// dispatcher, and the agent tools. The agent list's settings stay on this
// phone; the others keep where they kept before.

import { useI18n } from "@maxhq/shared/i18n";

import { ScrollView, StyleSheet, View } from "react-native";

import type { Screens, ScreenProps } from "../../navigation";
import { PushSection } from "../../push/PushSection";
import { color } from "../../theme";
import { Card, CardRow } from "../../ui/Card";
import { Header } from "../../ui/Header";
import { Toggle } from "../../ui/Toggle";
import { SendSection } from "../agent/SendSection";
import { DispatcherSection } from "../dispatcher/DispatcherSection";
import { keepInboxPrefs, useInboxPrefs } from "../inbox/prefs";
import { AgentToolsPage } from "./AgentTools";
import { SnoozeOptions } from "./SnoozeOptions";

export type SettingsPage = Screens["SettingsPage"]["page"];

/** Each page's title, for its header and the row that opens it. */
export const settingsPageTitle = (
	t: ReturnType<typeof useI18n>["t"],
	page: SettingsPage
): string => {
	switch (page) {
		case "agent-list":
			return t("Agent list");
		case "composer":
			return t("Composer");
		case "notifications":
			return t("Notifications");
		case "dispatcher":
			return t("Dispatcher");
		case "agent-tools":
			return t("Agent tools");
	}
};

const AgentList = () => {
	const { t } = useI18n();
	const workingShelf = useInboxPrefs((prefs) => prefs.workingShelf);
	return (
		<>
			<Card title={t("Shelves")}>
				<CardRow
					label={t("Working shelf")}
					note={t(
						"Fold busy agents into a Working shelf. They come back to the top when they need you."
					)}
				>
					<Toggle
						label={t("Working shelf")}
						value={workingShelf}
						onChange={(on) => keepInboxPrefs({ workingShelf: on })}
					/>
				</CardRow>
			</Card>
			<SnoozeOptions />
		</>
	);
};

const Body = ({ page }: { page: SettingsPage }) => {
	switch (page) {
		case "agent-list":
			return <AgentList />;
		case "composer":
			return <SendSection />;
		case "notifications":
			return <PushSection />;
		case "dispatcher":
			return <DispatcherSection />;
		case "agent-tools":
			return <AgentToolsPage />;
	}
};

export const SettingsPageScreen = ({ navigation, route }: ScreenProps<"SettingsPage">) => {
	const { t } = useI18n();
	const { page } = route.params;
	return (
		<View style={styles.screen}>
			<Header
				title={settingsPageTitle(t, page)}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
			/>
			<ScrollView
				contentContainerStyle={styles.content}
				keyboardShouldPersistTaps="handled"
				keyboardDismissMode="on-drag"
				automaticallyAdjustKeyboardInsets
			>
				<Body page={page} />
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
});
