// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A row of tabs drawn as the Mac switch's segments, each an equal share of
// the width: Open and Closed on the Pulls screen, a PR's four parts, and a
// task's requests and runs.

import { Pressable, StyleSheet, Text, View } from "react-native";

import { color, radius, size, tint, TOUCH } from "../../theme";

export type Tab<T extends string> = { readonly value: T; readonly label: string };

export const Tabs = <T extends string>({
	tabs,
	value,
	onChange,
}: {
	tabs: readonly Tab<T>[];
	value: T;
	onChange: (value: T) => void;
}) => (
	<View accessibilityRole="tablist" style={styles.row}>
		{tabs.map((tab) => {
			const on = tab.value === value;
			return (
				<Pressable
					key={tab.value}
					accessibilityRole="tab"
					accessibilityState={{ selected: on }}
					onPress={() => onChange(tab.value)}
					style={({ pressed }) => [
						styles.segment,
						on && styles.on,
						pressed && !on && styles.pressed,
					]}
				>
					<Text style={[styles.label, on && styles.onLabel]} numberOfLines={1}>
						{tab.label}
					</Text>
				</Pressable>
			);
		})}
	</View>
);

const styles = StyleSheet.create({
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
		flex: 1,
		minHeight: TOUCH - 4,
		alignItems: "center",
		justifyContent: "center",
		paddingHorizontal: 6,
		borderRadius: radius.sm,
	},
	on: { backgroundColor: color.accent },
	pressed: { backgroundColor: tint(color.foreground, 0.04) },
	label: { color: color.mutedForeground, fontSize: size.small, fontWeight: "500" },
	onLabel: { color: color.foregroundStrong },
});
