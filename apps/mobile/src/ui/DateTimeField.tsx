// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A labelled date or time field with the phone's own picker: on iOS the
// compact picker, which opens its calendar or clock in place; on Android a
// box with the value that opens the system's dialog. Given a zone, it shows
// and sets the time on that zone's clock rather than the phone's.

import { formatDate } from "@maxhq/shared/i18n";

import DateTimePicker, { DateTimePickerAndroid } from "@react-native-community/datetimepicker";
import { Platform, Pressable, StyleSheet, Text, View } from "react-native";

import { knownZone } from "@maxhq/shared/lib/zoned-time";

import { color, radius, size, TOUCH } from "../theme";

export const DateTimeField = ({
	label,
	mode,
	value,
	onChange,
	zone,
	minimumDate,
}: {
	label: string;
	mode: "date" | "time";
	value: Date;
	/** Takes the picked moment: a new day at the same time, or a new time on the same day. */
	onChange: (value: Date) => void;
	/** The zone whose clock it shows, such as `Europe/Amsterdam`; the phone's when left out. */
	zone?: string;
	minimumDate?: Date;
}) => {
	const timeZoneName = zone === undefined ? undefined : knownZone(zone);
	const field =
		Platform.OS === "ios" ? (
			<DateTimePicker
				accessibilityLabel={label}
				value={value}
				mode={mode}
				display="compact"
				timeZoneName={timeZoneName}
				minimumDate={minimumDate}
				themeVariant="dark"
				accentColor={color.primary}
				onValueChange={(_, picked) => onChange(picked)}
			/>
		) : (
			<Pressable
				accessibilityRole="button"
				accessibilityLabel={label}
				onPress={() =>
					DateTimePickerAndroid.open({
						value,
						mode,
						timeZoneName,
						minimumDate,
						onValueChange: (_, picked) => onChange(picked),
					})
				}
				style={({ pressed }) => [styles.box, pressed && styles.pressed]}
			>
				<Text style={styles.value}>
					{formatDate(
						value,
						mode === "date"
							? {
									day: "numeric",
									month: "short",
									year: "numeric",
									timeZone: timeZoneName,
								}
							: { hour: "numeric", minute: "2-digit", timeZone: timeZoneName }
					)}
				</Text>
			</Pressable>
		);
	return (
		<View style={styles.field}>
			<Text style={styles.label}>{label}</Text>
			<View style={styles.picker}>{field}</View>
		</View>
	);
};

const styles = StyleSheet.create({
	field: { gap: 6 },
	label: { color: color.secondaryForeground, fontSize: size.meta, fontWeight: "500" },
	picker: { minHeight: TOUCH, alignItems: "flex-start", justifyContent: "center" },
	box: {
		minHeight: TOUCH,
		minWidth: 120,
		justifyContent: "center",
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.subpanel,
		paddingHorizontal: 12,
	},
	pressed: { backgroundColor: color.accent },
	value: { color: color.foreground, fontSize: size.body },
});
