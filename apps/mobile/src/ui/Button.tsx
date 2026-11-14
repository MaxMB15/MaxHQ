// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A button as the desktop draws it: blue for the main choice, a bordered
// dark one for the others, and the same with red text for a no. A small
// one still takes a 44 point touch.

import type { ReactNode } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { Tone } from "../features/attention/buttons";
import { color, radius, size, TOUCH } from "../theme";

const TONE = {
	primary: { box: color.primary, border: color.primary, text: color.primaryForeground },
	secondary: { box: color.secondary, border: color.input, text: color.foreground },
	danger: { box: color.secondary, border: color.input, text: color.errorText },
	link: { box: "transparent", border: "transparent", text: color.primary },
} as const;

const HEIGHT = { md: TOUCH, sm: 32 } as const;

export const Button = ({
	label,
	tone = "secondary",
	small,
	grow,
	icon,
	hint,
	disabled,
	onPress,
}: {
	label: string;
	tone?: Tone | "link";
	/** 32 tall, for a button beside a row's text. */
	small?: boolean;
	/** How much of a row's width it takes, beside its other buttons. */
	grow?: number;
	icon?: ReactNode;
	/** What it does, when its label doesn't say it all, which VoiceOver reads after it. */
	hint?: string;
	disabled?: boolean;
	onPress: () => void;
}) => {
	const height = HEIGHT[small ? "sm" : "md"];
	const slop = (TOUCH - height) / 2;
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={label}
			accessibilityHint={hint}
			accessibilityState={{ disabled: Boolean(disabled) }}
			disabled={disabled}
			onPress={onPress}
			hitSlop={slop > 0 ? { top: slop, bottom: slop } : undefined}
			style={({ pressed }) => [
				styles.box,
				{
					height,
					paddingHorizontal: small ? 12 : 14,
					backgroundColor: TONE[tone].box,
					borderColor: TONE[tone].border,
				},
				grow !== undefined && { flexGrow: grow, flexBasis: 0 },
				pressed && (tone === "primary" ? styles.pressedPrimary : styles.pressed),
				disabled && styles.dim,
			]}
		>
			{icon ? <View>{icon}</View> : null}
			<Text
				numberOfLines={1}
				style={[
					styles.label,
					{ color: TONE[tone].text, fontSize: small ? 12.5 : 13.5 },
					tone === "primary" && styles.strong,
				]}
			>
				{label}
			</Text>
		</Pressable>
	);
};

const styles = StyleSheet.create({
	box: {
		flexDirection: "row",
		gap: 8,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		alignItems: "center",
		justifyContent: "center",
	},
	pressed: { backgroundColor: color.accent },
	pressedPrimary: { opacity: 0.85 },
	dim: { opacity: 0.55 },
	label: { fontSize: size.small, fontWeight: "500" },
	strong: { fontWeight: "600" },
});
