// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The switch at the top of the agent list while several Macs show one at a
// time: a segment per Mac, each with how much waits on you there, counted
// across every Mac so the ones you don't see still say so.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";

import type { ViewState } from "@maxhq/shared/view/replica";

import { color, radius, size, tint, TOUCH } from "../../theme";
import { macSegments } from "./macs";
import { keepMacsPref } from "./pref";
import type { Macs } from "./useMacs";

export const MacSwitch = ({ view, macs }: { view: ViewState; macs: Macs }) => {
	const { t } = useI18n();
	if (macs.mode.kind !== "one") return null;
	const picked = macs.mode.picked.keeper;
	return (
		<ScrollView
			horizontal
			showsHorizontalScrollIndicator={false}
			accessibilityRole="tablist"
			contentContainerStyle={styles.row}
			style={styles.bar}
		>
			{macSegments(view, macs.places, macs.macs).map(({ mac, label, waiting }) => {
				const on = mac.keeper === picked;
				return (
					<Pressable
						key={mac.keeper}
						accessibilityRole="tab"
						accessibilityState={{ selected: on }}
						accessibilityLabel={
							waiting > 0
								? t("{{mac}}, {{count}} waiting on you", {
										mac: label,
										count: waiting,
									})
								: label
						}
						onPress={() => keepMacsPref({ pick: mac.keeper })}
						style={({ pressed }) => [
							styles.segment,
							on && styles.on,
							pressed && !on && styles.pressed,
						]}
					>
						<Text style={[styles.label, on && styles.onLabel]} numberOfLines={1}>
							{label}
						</Text>
						{waiting > 0 ? (
							<View style={styles.count}>
								<Text style={styles.countText}>{formatNumber(waiting)}</Text>
							</View>
						) : null}
					</Pressable>
				);
			})}
		</ScrollView>
	);
};

const styles = StyleSheet.create({
	bar: { flexGrow: 0, marginBottom: 12 },
	row: {
		flexDirection: "row",
		gap: 2,
		padding: 2,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
	},
	segment: {
		minHeight: TOUCH - 4,
		minWidth: 64,
		flexDirection: "row",
		alignItems: "center",
		justifyContent: "center",
		gap: 6,
		paddingHorizontal: 14,
		borderRadius: radius.sm,
	},
	on: { backgroundColor: color.accent },
	pressed: { backgroundColor: tint(color.foreground, 0.04) },
	label: { color: color.mutedForeground, fontSize: size.small, fontWeight: "500" },
	onLabel: { color: color.foregroundStrong },
	count: {
		minWidth: 16,
		height: 16,
		paddingHorizontal: 4,
		borderRadius: 8,
		backgroundColor: tint(color.ready, 0.18),
		alignItems: "center",
		justifyContent: "center",
	},
	countText: { color: color.readyText, fontSize: size.pill, fontWeight: "600" },
});
