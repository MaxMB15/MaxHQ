// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual in a feed or a note, as one row: its diamond and title, and Open,
// which shows it whole. The page runs only once it's open.

import { useI18n } from "@maxhq/shared/i18n";

import { Pressable, StyleSheet, Text } from "react-native";

import { color, radius, size } from "../../theme";
import { openVisual, type VisualToOpen, visualName } from "./visual";

/** A visual's row, which opens it whole. */
export const VisualOpenRow = ({ visual, meta }: { visual: VisualToOpen; meta?: string | null }) => {
	const { t } = useI18n();

	const name = visualName(visual.title);
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={t("Open the visual {{name}}", { name })}
			onPress={() => openVisual(visual)}
			style={({ pressed }) => [styles.row, pressed ? styles.pressed : null]}
		>
			<Text style={styles.diamond}>◆</Text>
			<Text numberOfLines={1} style={styles.title}>
				{name}
			</Text>
			{meta ? <Text style={styles.meta}>{meta}</Text> : null}
			<Text style={styles.dot}>·</Text>
			<Text style={styles.open}>{t("Open")}</Text>
		</Pressable>
	);
};

const styles = StyleSheet.create({
	row: {
		minHeight: 32,
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		alignSelf: "flex-start",
		maxWidth: "100%",
		paddingHorizontal: 8,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
	},
	pressed: { backgroundColor: color.accent },
	diamond: { color: color.visual, fontSize: size.small },
	title: { flexShrink: 1, color: color.visual, fontSize: size.small, fontWeight: "600" },
	meta: { flexShrink: 0, color: color.mutedForeground, fontSize: size.meta },
	dot: { color: color.mutedForeground, fontSize: size.small },
	open: { color: color.visual, fontSize: size.small },
});
