// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Text boxes as the desktop draws them: a plain one, and one with its
// button on its end, as the box for a reason with Deny beside it.

import { type ComponentRef, forwardRef } from "react";
import { Pressable, StyleSheet, Text, TextInput, type TextInputProps, View } from "react-native";

import { color, radius, size, TOUCH } from "../theme";

/** A 44 point text box on the dark panel color. */
export const Input = forwardRef<ComponentRef<typeof TextInput>, TextInputProps>(
	({ style, ...props }, ref) => (
		<TextInput
			ref={ref}
			placeholderTextColor={color.subtleForeground}
			{...props}
			style={[styles.input, style]}
		/>
	)
);
Input.displayName = "Input";

/** A text box with its button on its end. */
export const AttachedField = ({
	value,
	onChange,
	placeholder,
	label,
	button,
	danger,
	disabled,
	needsText,
	onSubmit,
}: {
	value: string;
	onChange: (text: string) => void;
	placeholder: string;
	/** What VoiceOver reads for the box. */
	label: string;
	button: string;
	/** Red button text, for a no. */
	danger?: boolean;
	disabled?: boolean;
	/** Set when the button does nothing until there's text. */
	needsText?: boolean;
	onSubmit: () => void;
}) => {
	const off = disabled || (needsText && value.trim() === "");
	return (
		<View style={styles.attached}>
			<TextInput
				style={styles.inner}
				value={value}
				onChangeText={onChange}
				placeholder={placeholder}
				placeholderTextColor={color.subtleForeground}
				accessibilityLabel={label}
				editable={!disabled}
				returnKeyType="send"
				onSubmitEditing={() => !off && onSubmit()}
			/>
			<Pressable
				accessibilityRole="button"
				accessibilityState={{ disabled: Boolean(off) }}
				disabled={off}
				onPress={onSubmit}
				style={({ pressed }) => [styles.end, pressed && styles.pressed, off && styles.dim]}
			>
				<Text style={[styles.endLabel, danger && styles.danger]}>{button}</Text>
			</Pressable>
		</View>
	);
};

const styles = StyleSheet.create({
	input: {
		minHeight: TOUCH,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.subpanel,
		paddingHorizontal: 12,
		paddingVertical: 10,
		color: color.foreground,
		fontSize: size.body,
	},
	attached: {
		height: TOUCH,
		flexDirection: "row",
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.background,
		overflow: "hidden",
	},
	inner: {
		flex: 1,
		minWidth: 0,
		paddingHorizontal: 10,
		color: color.foreground,
		fontSize: size.small,
	},
	end: {
		justifyContent: "center",
		paddingHorizontal: 14,
		borderLeftWidth: StyleSheet.hairlineWidth,
		borderLeftColor: color.input,
		backgroundColor: color.secondary,
	},
	pressed: { backgroundColor: color.accent },
	dim: { opacity: 0.55 },
	endLabel: { color: color.foreground, fontSize: size.small, fontWeight: "500" },
	danger: { color: color.errorText },
});
