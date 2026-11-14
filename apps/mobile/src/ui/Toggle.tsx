// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// An on and off switch as the desktop draws it: a small blue track when
// on, grey when off, in a 44 point touch.

import { Pressable, StyleSheet, View } from "react-native";

import { color, TOUCH } from "../theme";

export const Toggle = ({
	label,
	value,
	onChange,
}: {
	label: string;
	value: boolean;
	onChange: (on: boolean) => void;
}) => (
	<Pressable
		accessibilityRole="switch"
		accessibilityLabel={label}
		accessibilityState={{ checked: value }}
		onPress={() => onChange(!value)}
		style={styles.touch}
	>
		<View style={[styles.track, value ? styles.on : styles.off]}>
			<View
				style={[
					styles.knob,
					{ backgroundColor: value ? color.foregroundStrong : color.mutedForeground },
				]}
			/>
		</View>
	</Pressable>
);

const styles = StyleSheet.create({
	touch: { width: TOUCH, height: TOUCH, alignItems: "flex-end", justifyContent: "center" },
	track: { width: 34, height: 20, borderRadius: 10, padding: 2 },
	on: { backgroundColor: color.primary, alignItems: "flex-end" },
	off: { backgroundColor: color.input, alignItems: "flex-start" },
	knob: { width: 16, height: 16, borderRadius: 8 },
});
