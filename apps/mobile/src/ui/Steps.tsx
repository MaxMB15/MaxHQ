// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Numbered steps, each number in a small square.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";

import { color, size } from "../theme";

export const Steps = ({ steps }: { steps: readonly string[] }) => {
	useI18n();
	return (
		<View style={styles.list} accessibilityRole="list">
			{steps.map((step, i) => (
				<View
					key={step}
					style={styles.step}
					accessible
					accessibilityLabel={`${formatNumber(i + 1)}. ${step}`}
				>
					<View style={styles.number}>
						<Text style={styles.digit}>{formatNumber(i + 1)}</Text>
					</View>
					<Text style={styles.text}>{step}</Text>
				</View>
			))}
		</View>
	);
};

const styles = StyleSheet.create({
	list: { gap: 8 },
	step: { flexDirection: "row", alignItems: "flex-start", gap: 10 },
	number: {
		width: 20,
		height: 20,
		borderRadius: 6,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
		alignItems: "center",
		justifyContent: "center",
	},
	digit: { color: color.secondaryForeground, fontSize: size.pill, fontWeight: "600" },
	text: { flex: 1, color: color.terminal, fontSize: size.small, lineHeight: 19 },
});
