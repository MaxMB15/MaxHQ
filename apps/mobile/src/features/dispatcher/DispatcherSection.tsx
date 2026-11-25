// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatcher's settings on this phone: whether its bar always shows on
// the home screen, and which of the Mac's accounts it runs as.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text } from "react-native";
import { useStore } from "zustand";

import { dispatcherAccountLabel, dispatcherAccounts } from "@maxhq/shared/dispatch/plan";
import { dispatcherReach } from "@maxhq/shared/dispatch/reach";
import { accountName } from "@maxhq/shared/status/usage";

import { useView } from "../../services";
import { color, size } from "../../theme";
import { Card, CardRow } from "../../ui/Card";
import { choose } from "../../ui/choose";
import { BAR_SETTINGS, barSettingLabel } from "./bar";
import { dispatcherPrefs, keepAccount, keepBar } from "./prefs";

export const DispatcherSection = () => {
	const { t } = useI18n();

	const prefs = useStore(dispatcherPrefs);
	const view = useView((s) => s.view);
	const reach = dispatcherReach(view, prefs.account);
	const accounts = dispatcherAccounts(view, reach.device);

	const pickBar = () =>
		choose(
			t("Dispatcher bar"),
			BAR_SETTINGS.map(({ setting, label }) => ({
				label: t(label),
				pick: () => keepBar(setting),
			}))
		);

	const pickAccount = () =>
		choose(
			t("Runs as"),
			accounts.map((a) => ({
				label: dispatcherAccountLabel(a, view.tables.model_server),
				pick: () => keepAccount(a.id),
			}))
		);

	return (
		<Card
			title={t("Dispatcher")}
			note={t(
				"Ask from the bar on the home screen. The dispatcher runs on the Mac, and nothing it plans runs until you confirm."
			)}
		>
			<CardRow
				label={t("Dispatcher bar")}
				note={prefs.bar === "in_use" ? t("Swipe up from the bottom to open it") : undefined}
				onPress={pickBar}
			>
				<Text style={styles.value} numberOfLines={1}>
					{barSettingLabel(prefs.bar)}
				</Text>
			</CardRow>
			<CardRow
				label={t("Runs as")}
				note={
					reach.account
						? t(
								"Without a pick, the Mac's first Claude account, else its first Codex one"
							)
						: (reach.off ?? undefined)
				}
				onPress={accounts.length > 1 ? pickAccount : undefined}
			>
				<Text style={styles.value} numberOfLines={1}>
					{reach.account
						? accountName(reach.account, view.tables.model_server)
						: t("None")}
				</Text>
			</CardRow>
		</Card>
	);
};

const styles = StyleSheet.create({
	value: { maxWidth: "50%", color: color.secondaryForeground, fontSize: size.small },
});
