// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A settings card, as the desktop's settings sections: a title and a note,
// then rows split by thin lines, each with its label and what it controls.

import type { ReactNode } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { color, radius, size, TOUCH } from "../theme";
import { Chevron } from "./Icon";

export const Card = ({
	title,
	note,
	label,
	children,
}: {
	title?: string;
	note?: string;
	/** What VoiceOver reads for the card when it has no title. */
	label?: string;
	children: ReactNode;
}) => (
	<View style={styles.card} accessibilityLabel={label}>
		{title ? (
			<View style={styles.head}>
				<Text accessibilityRole="header" style={styles.title}>
					{title}
				</Text>
				{note ? <Text style={styles.note}>{note}</Text> : null}
			</View>
		) : null}
		{children}
	</View>
);

/** One row of a card: a label with a note under it, and a control or value. */
export const CardRow = ({
	label,
	note,
	first,
	children,
	onPress,
}: {
	label: string;
	note?: string;
	/** Set on a card's first row when the card has no title over it. */
	first?: boolean;
	children?: ReactNode;
	/** Makes the row a link, with a chevron at its end. */
	onPress?: () => void;
}) => {
	const body = (
		<>
			<View style={styles.text}>
				<Text style={styles.label}>{label}</Text>
				{note ? <Text style={styles.note}>{note}</Text> : null}
			</View>
			{children}
			{onPress ? <Chevron dir="right" size={12} /> : null}
		</>
	);
	const line = [styles.row, !first && styles.line];
	if (!onPress) return <View style={line}>{body}</View>;
	return (
		<Pressable
			accessibilityRole="link"
			onPress={onPress}
			style={({ pressed }) => [...line, styles.link, pressed && styles.pressed]}
		>
			{body}
		</Pressable>
	);
};

const styles = StyleSheet.create({
	card: {
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.track,
		backgroundColor: color.card,
		overflow: "hidden",
	},
	head: { paddingHorizontal: 14, paddingVertical: 12, gap: 2 },
	title: { color: color.foregroundStrong, fontSize: 13.5, fontWeight: "700" },
	row: {
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "center",
		gap: 12,
		paddingHorizontal: 14,
		paddingVertical: 10,
	},
	line: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	link: { paddingRight: 12 },
	pressed: { backgroundColor: color.accent },
	text: { flex: 1, gap: 1 },
	label: { color: color.foreground, fontSize: size.small },
	note: { color: color.mutedForeground, fontSize: size.meta },
});
