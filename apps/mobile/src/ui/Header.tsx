// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A screen's compact header, as the desktop's pane headers: a back or close
// button, the title with a line under it, and icon buttons on the right.
// The first screen shows a bigger title and no back. It sits under the
// status bar, or at the top of a sheet, which has none.

import type { ReactNode } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { color, size, TOUCH } from "../theme";
import { Chevron, Close } from "./Icon";

/** A 44 point square button with an icon in it. */
export const IconButton = ({
	label,
	onPress,
	disabled,
	children,
}: {
	label: string;
	onPress: () => void;
	disabled?: boolean;
	children: ReactNode;
}) => (
	<Pressable
		accessibilityRole="button"
		accessibilityLabel={label}
		accessibilityState={{ disabled: Boolean(disabled) }}
		disabled={disabled}
		onPress={onPress}
		style={({ pressed }) => [styles.icon, pressed && styles.pressed, disabled && styles.off]}
	>
		{children}
	</Pressable>
);

export const Header = ({
	title,
	subtitle,
	back,
	right,
	sheet,
}: {
	title: string;
	/** A line under the title, such as an agent's state. */
	subtitle?: ReactNode;
	/** The button on the left, and what it's for. */
	back?: { readonly label: string; readonly onPress: () => void; readonly close?: boolean };
	right?: ReactNode;
	/** Set on a sheet, which starts below the status bar. */
	sheet?: boolean;
}) => {
	const insets = useSafeAreaInsets();
	return (
		<View style={[styles.bar, { paddingTop: sheet ? 0 : insets.top }]}>
			<View style={[styles.row, !back && styles.first]}>
				{back ? (
					<IconButton label={back.label} onPress={back.onPress}>
						{back.close ? (
							<Close />
						) : (
							<Chevron dir="left" size={20} tint={color.secondaryForeground} />
						)}
					</IconButton>
				) : null}
				<View style={styles.titles}>
					<Text
						accessibilityRole="header"
						numberOfLines={1}
						style={back ? styles.title : styles.big}
					>
						{title}
					</Text>
					{subtitle}
				</View>
				{right}
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	bar: {
		backgroundColor: color.background,
		borderBottomColor: color.border,
		borderBottomWidth: StyleSheet.hairlineWidth,
	},
	row: { height: 52, flexDirection: "row", alignItems: "center", gap: 2, paddingHorizontal: 6 },
	first: { paddingLeft: 16, gap: 6 },
	titles: { flex: 1, minWidth: 0 },
	title: { color: color.foregroundStrong, fontSize: size.heading, fontWeight: "600" },
	big: {
		color: color.foregroundStrong,
		fontSize: size.title,
		fontWeight: "700",
		letterSpacing: -0.2,
	},
	icon: {
		width: TOUCH,
		height: TOUCH,
		alignItems: "center",
		justifyContent: "center",
		borderRadius: 8,
	},
	pressed: { backgroundColor: color.accent },
	off: { opacity: 0.4 },
});
