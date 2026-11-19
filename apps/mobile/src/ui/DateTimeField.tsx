// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A labelled date or time field with the phone's own picker, on the
// phone's clock: on iOS the compact picker, which opens its calendar or
// clock in place and names months and days in MaxHQ's language with the
// phone's region; on Android a box with the value in MaxHQ's language that
// opens the system's dialog, which Android draws in the phone's language.

import { formatDate, regionalLocale, useI18n } from "@maxhq/shared/i18n";

import DateTimePicker, { DateTimePickerAndroid } from "@react-native-community/datetimepicker";
import { useLocales } from "expo-localization";
import { Platform, Pressable, StyleSheet, Text, View } from "react-native";

import { color, radius, size, TOUCH } from "../theme";

export const DateTimeField = ({
	label,
	mode,
	value,
	onChange,
	minimumDate,
}: {
	label: string;
	mode: "date" | "time";
	value: Date;
	/** Takes the picked moment: a new day at the same time, or a new time on the same day. */
	onChange: (value: Date) => void;
	minimumDate?: Date;
}) => {
	const { language } = useI18n();
	// The Region setting, which can differ from the region of the phone's language.
	const region = useLocales()[0].regionCode;
	const field =
		Platform.OS === "ios" ? (
			<DateTimePicker
				accessibilityLabel={label}
				value={value}
				mode={mode}
				display="compact"
				locale={regionalLocale(language, region)}
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
							? { day: "numeric", month: "short", year: "numeric" }
							: { hour: "numeric", minute: "2-digit" }
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
