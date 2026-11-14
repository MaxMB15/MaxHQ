// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the inbox shows before the phone is paired with any device: what
// pairing gives you, the steps on the Mac, and the button to scan its code.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";

import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { Corners } from "../../ui/Icon";
import { Steps } from "../../ui/Steps";

export const FirstRun = ({ onScan, onByHand }: { onScan: () => void; onByHand: () => void }) => {
	const { t } = useI18n();
	return (
		<View style={styles.screen}>
			<View style={styles.intro}>
				<View style={styles.tile}>
					<Corners size={22} tint={color.secondaryForeground} />
				</View>
				<Text accessibilityRole="header" style={styles.title}>
					{t("Pair this phone with your Mac")}
				</Text>
				<Text style={styles.body}>
					{t(
						"Once paired, you see the agents on the Mac and on every device it reaches, and you can answer them from here."
					)}
				</Text>
			</View>
			<Card label={t("On the Mac")}>
				<View style={styles.steps}>
					<Text style={styles.label}>{t("On the Mac")}</Text>
					<Steps
						steps={[
							t("Open MaxHQ, then Settings, then Phones."),
							t("Choose Add phone. A code shows."),
							t("Scan it with this phone."),
						]}
					/>
				</View>
			</Card>
			<View style={styles.actions}>
				<Button
					label={t("Scan the code")}
					tone="primary"
					icon={<Corners size={14} tint={color.primaryForeground} />}
					onPress={onScan}
				/>
				<Button label={t("Add a device by hand")} tone="link" onPress={onByHand} />
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: {
		flex: 1,
		justifyContent: "center",
		gap: 20,
		paddingHorizontal: 20,
		paddingBottom: 40,
	},
	intro: { alignItems: "center", gap: 14 },
	tile: {
		width: 52,
		height: 52,
		borderRadius: 14,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.track,
		backgroundColor: color.card,
		alignItems: "center",
		justifyContent: "center",
	},
	title: { color: color.foregroundStrong, fontSize: 18, fontWeight: "700", textAlign: "center" },
	body: {
		maxWidth: 300,
		color: color.mutedForeground,
		fontSize: 13.5,
		lineHeight: 19,
		textAlign: "center",
	},
	steps: { paddingHorizontal: 14, paddingVertical: 12, gap: 9 },
	label: { color: color.mutedForeground, fontSize: size.meta, fontWeight: "600" },
	actions: { gap: 8 },
});
