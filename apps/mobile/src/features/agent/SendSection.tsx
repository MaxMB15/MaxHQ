// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the send button does on this phone while an agent works. The button
// beside it does the other, once you type.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text } from "react-native";
import { useStore } from "zustand";

import { BUSY_SEND_WORDS, type BusySend } from "@maxhq/shared/composer/send";

import { color, size } from "../../theme";
import { Card, CardRow } from "../../ui/Card";
import { choose } from "../../ui/choose";
import { busySendPref, keepBusySend } from "./send-pref";

const PICKS: readonly BusySend[] = ["queue", "steer"];

export const SendSection = () => {
	const { t } = useI18n();

	const pick = useStore(busySendPref, (s) => s.pick);
	return (
		<Card title={t("Composer")}>
			<CardRow
				label={t("While an agent works")}
				note={BUSY_SEND_WORDS[pick].line}
				onPress={() =>
					choose(
						t("While an agent works"),
						PICKS.map((value) => ({
							label: BUSY_SEND_WORDS[value].label,
							disabled: value === pick,
							pick: () => keepBusySend(value),
						})),
						t("The send button does this. The other shows beside it once you type.")
					)
				}
			>
				<Text style={styles.value} numberOfLines={1}>
					{BUSY_SEND_WORDS[pick].label}
				</Text>
			</CardRow>
		</Card>
	);
};

const styles = StyleSheet.create({
	value: { maxWidth: "50%", color: color.secondaryForeground, fontSize: size.small },
});
