// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The small dialog that sets an agent's effort level: the levels its model
// takes side by side, low to high, with the one it runs at filled and the
// one its model runs at when told none marked Default. A tap applies the
// level at once and closes the dialog, as the desktop's slider does.

import { useI18n } from "@maxhq/shared/i18n";

import { Modal, Pressable, StyleSheet, Text, View } from "react-native";

import { color, radius, size, TOUCH } from "../../theme";
import { Button } from "../../ui/Button";
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
	const marked = pick?.levels.some((level) => level.isDefault) ?? false;
	return (
		<Modal visible={pick !== null} transparent animationType="fade" onRequestClose={close}>
			<View style={styles.scrim}>
				<Pressable accessible={false} style={StyleSheet.absoluteFill} onPress={close} />
				<View style={styles.card} accessibilityViewIsModal>
					<View style={styles.head}>
						<Text style={styles.title} accessibilityRole="header">
							{t("Effort")}
						</Text>
						<Text style={styles.message}>{pick?.message}</Text>
					</View>
					<View style={styles.track} accessibilityRole="radiogroup">
						{pick?.levels.map((level) => {
							const on = level.value === pick.current;
							return (
								<Pressable
									key={level.value}
									accessibilityRole="radio"
									accessibilityLabel={
										level.isDefault
											? t("{{level}} (default)", { level: level.label })
											: level.label
									}
									accessibilityState={{ checked: on }}
									onPress={() => {
										close();
										if (!on) onPick(level.value);
									}}
									style={[styles.segment, on && styles.on]}
								>
									<Text
										numberOfLines={1}
										adjustsFontSizeToFit
										minimumFontScale={0.8}
										style={[styles.label, on && styles.onLabel]}
									>
										{level.label}
									</Text>
								</Pressable>
							);
						})}
					</View>
					{marked ? (
						<View style={styles.captions}>
							{pick?.levels.map((level) => (
								<Text key={level.value} style={styles.caption}>
									{level.isDefault ? t("Default") : ""}
								</Text>
							))}
						</View>
					) : null}
					<Button label={t("Cancel")} onPress={close} />
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
	track: {
		flexDirection: "row",
		padding: 2,
		gap: 2,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
	},
	segment: {
		flex: 1,
		minHeight: TOUCH - 4,
		alignItems: "center",
		justifyContent: "center",
		paddingHorizontal: 2,
		borderRadius: radius.sm,
	},
	on: { backgroundColor: color.primary },
	label: { color: color.secondaryForeground, fontSize: size.meta },
	onLabel: { color: color.primaryForeground, fontWeight: "600" },
	captions: { flexDirection: "row", marginTop: -8, paddingHorizontal: 2, gap: 2 },
	caption: {
		flex: 1,
		textAlign: "center",
		color: color.mutedForeground,
		fontSize: size.pill,
	},
});
