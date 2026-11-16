// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The card a code block and a diagram sit in, as on the desktop: a header
// with the language and its buttons, then the code in a small monospaced
// face that scrolls sideways.

import { useI18n } from "@maxhq/shared/i18n";

import * as Clipboard from "expo-clipboard";
import type { ReactNode } from "react";
import { Pressable, type PressableProps, ScrollView, StyleSheet, Text, View } from "react-native";

import { copyLabel, useCopyWith } from "@maxhq/shared/lib/copy";

import { color, font, radius, size, TOUCH } from "../theme";
import { Check, CircleAlert, Copy } from "./glyphs";

const HEADER = 30;
const BUTTON = 22;

// A header button reaches a finger's height, and half the gap to its neighbour.
const SLOP = { top: (TOUCH - BUTTON) / 2, bottom: (TOUCH - BUTTON) / 2, left: 2, right: 2 };

/** A button in a card's header. */
export const HeaderButton = ({
	children,
	...props
}: Omit<PressableProps, "style" | "hitSlop" | "children"> & { children: ReactNode }) => (
	<Pressable
		accessibilityRole="button"
		hitSlop={SLOP}
		{...props}
		style={({ pressed }) => [cardStyles.button, pressed ? styles.pressed : null]}
	>
		{children}
	</Pressable>
);

const COPY_ICON = { idle: Copy, copied: Check, failed: CircleAlert } as const;

/** Copies `text`, and says for a moment that it did. */
export const CopyButton = ({ text }: { text: string }) => {
	const { t } = useI18n();
	const [state, copy] = useCopyWith(Clipboard.setStringAsync);
	const Icon = COPY_ICON[state];
	return (
		<HeaderButton onPress={() => copy(text)}>
			<Icon size={12} strokeWidth={1.9} color={color.mutedForeground} />
			<Text style={cardStyles.buttonText}>{copyLabel(state, t("Copy"))}</Text>
		</HeaderButton>
	);
};

/** A card with `name` and `buttons` in its header, over `children`. */
export const CodeCard = ({
	name,
	buttons,
	children,
}: {
	name: string;
	buttons?: ReactNode;
	children: ReactNode;
}) => (
	<View style={styles.card}>
		<View style={styles.header}>
			<Text numberOfLines={1} style={styles.name}>
				{name}
			</Text>
			{buttons}
		</View>
		{children}
	</View>
);

/** Code as a card shows it, selectable, scrolling sideways. */
export const CodeText = ({ children }: { children: ReactNode }) => (
	<ScrollView horizontal showsHorizontalScrollIndicator={false}>
		<Text selectable style={styles.code}>
			{children}
		</Text>
	</ScrollView>
);

/** The look of a header button, for one that isn't pressed. */
export const cardStyles = StyleSheet.create({
	button: {
		height: BUTTON,
		flexDirection: "row",
		alignItems: "center",
		gap: 5,
		paddingHorizontal: 6,
		borderRadius: radius.sm - 1,
	},
	buttonText: { color: color.mutedForeground, fontSize: size.tiny },
});

const styles = StyleSheet.create({
	card: {
		overflow: "hidden",
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
	},
	header: {
		height: HEADER,
		flexDirection: "row",
		alignItems: "center",
		gap: 4,
		paddingLeft: 12,
		paddingRight: 6,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.border,
	},
	name: {
		flex: 1,
		minWidth: 0,
		color: color.mutedForeground,
		fontFamily: font.mono,
		fontSize: size.tiny,
	},
	pressed: { backgroundColor: color.accent },
	code: {
		paddingHorizontal: 12,
		paddingVertical: 10,
		color: color.terminal,
		fontFamily: font.mono,
		fontSize: size.tiny,
		lineHeight: 18,
	},
});
