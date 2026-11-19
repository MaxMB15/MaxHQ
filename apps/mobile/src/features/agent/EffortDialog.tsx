// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The small dialog that sets an agent's effort level, on the slider the
// desktop's composer opens: the levels its model takes, low to high, with
// the one its model runs at when told none marked Default. A pick applies
// at once and the dialog stays open, as the desktop's popover does, so a
// drag or VoiceOver's swipes can move on to another level. Done, a tap
// outside or VoiceOver's escape closes it.

import { useI18n } from "@maxhq/shared/i18n";

import { Modal, Pressable, StyleSheet, Text, View } from "react-native";

import { color, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { StepSlider } from "../../ui/StepSlider";
import type { EffortPick } from "./more";

export const EffortDialog = ({
	pick,
	onPick,
	close,
}: {
	/** What to show, or null while it's closed. */
	pick: EffortPick | null;
	onPick: (effort: string) => void;
	close: () => void;
}) => {
	const { t } = useI18n();
	return (
		<Modal visible={pick !== null} transparent animationType="fade" onRequestClose={close}>
			<View style={styles.scrim}>
				<Pressable accessible={false} style={StyleSheet.absoluteFill} onPress={close} />
				<View style={styles.card} accessibilityViewIsModal onAccessibilityEscape={close}>
					<View style={styles.head}>
						<Text style={styles.title} accessibilityRole="header">
							{t("Effort")}
						</Text>
						<Text style={styles.message}>{pick?.message}</Text>
					</View>
					<StepSlider
						label={t("Effort")}
						stops={pick?.levels ?? []}
						value={pick?.current ?? ""}
						onPick={onPick}
					/>
					<Button label={t("Done")} onPress={close} />
				</View>
			</View>
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
		gap: 12,
		padding: 16,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.card,
	},
	head: { gap: 4 },
	title: { color: color.foregroundStrong, fontSize: size.heading, fontWeight: "600" },
	message: { color: color.mutedForeground, fontSize: size.meta },
});
