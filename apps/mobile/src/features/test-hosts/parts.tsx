// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Small pieces the test host views share: a host's state as a tinted word,
// and a switch that stays put while it's off limits, such as when the Mac
// hasn't let this phone change the device's settings.

import { StyleSheet, Text, View } from "react-native";

import type { Tone } from "@maxhq/shared/tasks/format";

import { color, size, tint } from "../../theme";
import { Toggle } from "../../ui/Toggle";

const TONE: Record<Tone, string> = {
	good: color.readyText,
	warn: color.busyText,
	bad: color.errorText,
	info: color.primary,
	plain: color.mutedForeground,
};

/** A state in a word or two, tinted by its tone. */
export const Badge = ({ label, tone }: { label: string; tone: Tone }) => (
	<View style={[styles.badge, { backgroundColor: tint(TONE[tone], 0.14) }]}>
		<Text numberOfLines={1} style={[styles.badgeText, { color: TONE[tone] }]}>
			{label}
		</Text>
	</View>
);

/** A switch that ignores taps and dims while `disabled`. */
export const GatedToggle = ({
	label,
	value,
	disabled,
	onChange,
}: {
	label: string;
	value: boolean;
	disabled: boolean;
	onChange: (on: boolean) => void;
}) => (
	<View
		pointerEvents={disabled ? "none" : "auto"}
		accessibilityState={{ disabled }}
		style={disabled && styles.dim}
	>
		<Toggle
			label={label}
			value={value}
			onChange={(on) => {
				if (!disabled) onChange(on);
			}}
		/>
	</View>
);

const styles = StyleSheet.create({
	badge: { borderRadius: 4, paddingHorizontal: 6, paddingVertical: 1, flexShrink: 0 },
	badgeText: { fontSize: size.pill, fontWeight: "600" },
	dim: { opacity: 0.4 },
});
