// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The plan usage meter in the agents header, as the desktop's: each
// vendor's 5-hour window, with the week as a lighter fill under it. A tap
// opens the Usage screen.

import { useI18n } from "@maxhq/shared/i18n";

import { Pressable, StyleSheet, Text, View } from "react-native";

import { formatPercent } from "@maxhq/shared/status/format";
import { type MeterRow, meterRows } from "@maxhq/shared/status/usage";
import { meterRowLabel } from "@maxhq/shared/status/usage-text";
import { accounts } from "@maxhq/shared/view/select";

import { useView } from "../../services";
import { color, radius, size } from "../../theme";
import { Bar } from "./Bar";

const Meter = ({ row }: { row: MeterRow }) => {
	return (
		<View style={styles.meter}>
			<Text style={styles.vendor}>{row.label}</Text>
			<Bar
				percent={row.percent}
				under={row.weekPercent}
				full={row.usedUp}
				style={styles.bar}
			/>
			<Text style={[styles.percent, row.usedUp && styles.usedUp]}>
				{row.percent === null ? "" : formatPercent(row.percent)}
			</Text>
		</View>
	);
};

export const UsageButton = ({ onPress }: { onPress: () => void }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const rows = meterRows(accounts(view));
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={
				rows.length > 0
					? t("Plan usage. {{summary}}", { summary: rows.map(meterRowLabel).join(", ") })
					: t("Plan usage")
			}
			hitSlop={{ top: 6, bottom: 6 }}
			onPress={onPress}
			style={({ pressed }) => [styles.button, pressed && styles.pressed]}
		>
			{rows.length > 0 ? (
				rows.map((row) => <Meter key={row.vendor} row={row} />)
			) : (
				<Text style={styles.label}>{t("Usage")}</Text>
			)}
		</Pressable>
	);
};

const styles = StyleSheet.create({
	button: {
		height: 32,
		justifyContent: "center",
		gap: 2,
		paddingHorizontal: 8,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
	},
	pressed: { backgroundColor: color.accent },
	meter: { flexDirection: "row", alignItems: "center", gap: 4 },
	vendor: { width: 36, color: color.secondaryForeground, fontSize: 10, lineHeight: 12 },
	bar: { width: 28, height: 3, borderRadius: 2 },
	percent: {
		width: 26,
		textAlign: "right",
		color: color.secondaryForeground,
		fontSize: 10,
		lineHeight: 12,
		fontVariant: ["tabular-nums"],
	},
	usedUp: { color: color.busy },
	label: { color: color.secondaryForeground, fontSize: size.meta, fontWeight: "500" },
});
