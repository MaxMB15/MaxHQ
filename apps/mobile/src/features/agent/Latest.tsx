// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The button that takes the feed back to its newest rows, shown while
// you're scrolled up, as the desktop's Jump to latest. It sits just over
// the box it's put in, the composer's. A dot says rows came in since you
// left the end.

import { useI18n } from "@maxhq/shared/i18n";

import { Pressable, StyleSheet, Text, View } from "react-native";

import { color, size } from "../../theme";
import { Chevron } from "../../ui/Icon";

export const Latest = ({ fresh, onPress }: { fresh: boolean; onPress: () => void }) => {
	const { t } = useI18n();
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={fresh ? t("Latest, with new activity") : t("Latest")}
			hitSlop={8}
			onPress={onPress}
			style={({ pressed }) => [styles.button, pressed && styles.pressed]}
		>
			{fresh ? <View style={styles.dot} /> : null}
			<Chevron dir="down" size={11} tint={color.foreground} />
			<Text style={styles.label}>{t("Latest")}</Text>
		</Pressable>
	);
};

const styles = StyleSheet.create({
	button: {
		position: "absolute",
		bottom: "100%",
		marginBottom: 10,
		alignSelf: "center",
		height: 30,
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		paddingHorizontal: 12,
		borderRadius: 15,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
	},
	pressed: { backgroundColor: color.accent },
	dot: { width: 7, height: 7, borderRadius: 3.5, backgroundColor: color.primary },
	label: { color: color.foreground, fontSize: size.meta, fontWeight: "500" },
});
