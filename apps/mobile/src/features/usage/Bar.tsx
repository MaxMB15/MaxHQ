// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A usage bar, as the desktop's: yellow once used up, and with an `under`
// figure, such as the week under the 5 hours, as a lighter fill beneath
// the main one, so it reads only past the main fill's end.

import { StyleSheet, View, type ViewStyle } from "react-native";

import { color } from "../../theme";

/** A percentage as a width, kept within the track. */
const width = (percent: number | null): `${number}%` =>
	`${Math.max(0, Math.min(100, percent ?? 0))}%`;

export const Bar = ({
	percent,
	full,
	under,
	style,
}: {
	percent: number | null;
	full: boolean;
	under?: number | null;
	/** The track's size and corners. */
	style: ViewStyle;
}) => {
	const fill = full ? color.busy : color.secondaryForeground;
	return (
		<View style={[styles.track, style]}>
			{under != null ? (
				<View style={[styles.under, { width: width(under), backgroundColor: fill }]} />
			) : null}
			<View style={[styles.main, { width: width(percent), backgroundColor: fill }]} />
		</View>
	);
};

const styles = StyleSheet.create({
	track: { overflow: "hidden", backgroundColor: color.track },
	under: { position: "absolute", top: 0, bottom: 0, left: 0, opacity: 0.35 },
	main: { height: "100%" },
});
