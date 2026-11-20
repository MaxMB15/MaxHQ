// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pieces a device's card on a project's settings page is made of: a
// setting with its label and line over its box, and a box with its unit
// after it, such as days. A box that takes no change now dims.

import type { ReactNode } from "react";
import { StyleSheet, Text, type TextInputProps, View } from "react-native";

import { color, size } from "../../theme";
import { Input } from "../../ui/Field";
import { CardBody } from "../devices/section";

/** A setting with its label and the line under it, then what sets it. */
export const Setting = ({
	label,
	note,
	children,
}: {
	label: string;
	note?: string;
	children: ReactNode;
}) => (
	<CardBody>
		<View style={styles.head}>
			<Text style={styles.label}>{label}</Text>
			{note ? <Text style={styles.note}>{note}</Text> : null}
		</View>
		{children}
	</CardBody>
);

/** A text box that dims while it takes no change. */
export const Box = ({
	editable,
	style,
	...props
}: TextInputProps & { readonly editable: boolean }) => (
	<Input
		{...props}
		editable={editable}
		autoCapitalize="none"
		autoCorrect={false}
		spellCheck={false}
		style={[style, !editable && styles.dim]}
	/>
);

/** A short number box with its unit after it. */
export const UnitBox = ({
	unit,
	...props
}: TextInputProps & { readonly editable: boolean; readonly unit: string }) => (
	<View style={styles.unit}>
		<Box {...props} style={styles.short} />
		<Text style={styles.note}>{unit}</Text>
	</View>
);

/** A label over a box, as the commit author's name and email. */
export const Labeled = ({ label, children }: { label: string; children: ReactNode }) => (
	<View style={styles.labeled}>
		<Text style={styles.small}>{label}</Text>
		{children}
	</View>
);

const styles = StyleSheet.create({
	head: { gap: 1 },
	label: { color: color.foreground, fontSize: size.small },
	note: { color: color.mutedForeground, fontSize: size.meta },
	small: { color: color.secondaryForeground, fontSize: size.meta, fontWeight: "500" },
	unit: { flexDirection: "row", alignItems: "center", gap: 8 },
	short: { width: 88, textAlign: "right", fontVariant: ["tabular-nums"] },
	labeled: { gap: 6 },
	dim: { opacity: 0.55 },
});
