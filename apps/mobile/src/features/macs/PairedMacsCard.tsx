// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Settings' Paired Macs card: each Mac the phone pairs with, its devices
// and Forget, then the way to pair with another Mac. With two or more
// Macs a card under it picks how they show: together or one at a time.

import { useI18n } from "@maxhq/shared/i18n";

import { useEffect, useState } from "react";
import { StyleSheet, Text } from "react-native";
import { useStore } from "zustand";

import { describe } from "@maxhq/shared/data/request-source";

import type { PairedMac, PhoneCalls } from "../../data/native-source";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { choose, confirm } from "../../ui/choose";
import { forgetWords, groupName, macRowNote, type MacsShow, severalMacs } from "./macs";
import { keepMacsPref, macsPref } from "./pref";
import { refreshMacs, useMacs } from "./useMacs";

export const PairedMacsCard = ({ phone, onPair }: { phone: PhoneCalls; onPair: () => void }) => {
	const { t } = useI18n();

	const { macs } = useMacs();
	const show = useStore(macsPref, (s) => s.show);
	const [failed, setFailed] = useState<string | null>(null);
	useEffect(() => refreshMacs(phone), [phone]);

	const paired = macs.some((mac) => !mac.by_hand);
	const forget = (mac: PairedMac) =>
		confirm(forgetWords(mac, macs), t("Forget"), () => {
			const result = phone.forgetMac(mac.keeper);
			setFailed(result.status === "ok" ? null : describe(result.error));
			refreshMacs(phone);
		});
	const shows: Record<MacsShow, string> = {
		together: t("Together"),
		one: t("One at a time"),
	};
	const pickShow = () =>
		choose(
			t("Show your Macs"),
			(Object.keys(shows) as MacsShow[]).map((value) => ({
				label: shows[value],
				disabled: show === value,
				pick: () => keepMacsPref({ show: value }),
			})),
			t(
				"Together tags each agent with its Mac. One at a time shows one Mac, with a switch at the top of the agent list."
			)
		);

	return (
		<>
			<Card title={t("Paired Macs")}>
				{macs.map((mac) => (
					<CardRow key={mac.keeper} label={groupName(mac)} note={macRowNote(mac)}>
						<Button
							label={t("Forget")}
							tone="danger"
							small
							onPress={() => forget(mac)}
						/>
					</CardRow>
				))}
				<CardRow
					label={paired ? t("Pair with another Mac") : t("Pair with a Mac")}
					onPress={onPair}
				/>
				{failed ? <Text style={[styles.note, styles.failed]}>{failed}</Text> : null}
				{paired ? (
					<Text style={styles.note}>
						{t(
							"Forget removes that Mac's devices from this phone. Your other Macs stay paired."
						)}
					</Text>
				) : null}
			</Card>
			{severalMacs(macs) ? (
				<Card title={t("Several Macs")}>
					<CardRow label={t("Show your Macs")} onPress={pickShow}>
						<Text style={styles.value}>{shows[show]}</Text>
					</CardRow>
				</Card>
			) : null}
		</>
	);
};

const styles = StyleSheet.create({
	value: { maxWidth: "50%", color: color.secondaryForeground, fontSize: size.small },
	note: {
		color: color.mutedForeground,
		fontSize: size.meta,
		paddingHorizontal: 14,
		paddingTop: 4,
		paddingBottom: 12,
	},
	failed: { color: color.errorText },
});
