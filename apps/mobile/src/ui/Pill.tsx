// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A small tinted label, as the desktop's Input pill and route chips.

import { StyleSheet, Text, View } from "react-native";

import { color, font, size, tint } from "../theme";

const TONE = {
	ready: { box: tint(color.ready, 0.16), border: "transparent", text: color.readyText },
	primary: {
		box: tint(color.primary, 0.14),
		border: tint(color.primary, 0.35),
		text: color.terminal,
	},
	plain: { box: color.secondary, border: color.input, text: color.mutedForeground },
} as const;

export const Pill = ({
	label,
	tone = "plain",
	mono,
	note,
}: {
	label: string;
	tone?: keyof typeof TONE;
	/** For a route or a path. */
	mono?: boolean;
	/** A word after it in the body font, such as "in use". */
	note?: string;
}) => (
	<View
		style={[styles.pill, { backgroundColor: TONE[tone].box, borderColor: TONE[tone].border }]}
	>
		<Text
			numberOfLines={1}
			style={[styles.label, { color: TONE[tone].text }, mono ? styles.mono : styles.strong]}
		>
			{label}
		</Text>
		{note ? <Text style={styles.note}>{note}</Text> : null}
	</View>
);

const styles = StyleSheet.create({
	pill: {
		flexDirection: "row",
		alignItems: "center",
		gap: 5,
		minHeight: 18,
		paddingHorizontal: 6,
		borderRadius: 4,
		borderWidth: StyleSheet.hairlineWidth,
		flexShrink: 1,
	},
	label: { fontSize: size.pill, flexShrink: 1 },
	strong: { fontWeight: "600" },
	mono: { fontFamily: font.mono, paddingVertical: 4 },
	note: { fontSize: size.pill, color: color.primary },
});
