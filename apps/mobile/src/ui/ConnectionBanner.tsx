// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The strip over a screen while the phone reaches none of its devices:
// offline when the phone has no network, reconnecting, or saved data, with
// when a device last answered. It's tinted in the color of the connection
// dot that says the same.

import { useI18n } from "@maxhq/shared/i18n";

import { useNetInfo } from "@react-native-community/netinfo";
import { StyleSheet, Text, View } from "react-native";

import { useNow } from "@maxhq/shared/lib/clock";
import { phoneOnline } from "@maxhq/shared/status/phone-network";
import { connectionBanner } from "@maxhq/shared/status/reach";

import { useView } from "../services";
import { color, radius, size, tint } from "../theme";
import { ConnectionDot } from "./Dot";

const TONE = {
	offline: { look: "unreachable", hue: color.error, text: color.errorText },
	reconnecting: { look: "connecting", hue: color.connecting, text: color.connectingText },
	cached: { look: "unknown", hue: color.off, text: color.secondaryForeground },
} as const;

export const ConnectionBanner = () => {
	useI18n();

	const view = useView((s) => s.view);
	const now = useNow(10_000);
	const online = phoneOnline(useNetInfo());
	const banner = connectionBanner(view, { online, now });
	if (!banner) return null;
	const tone = TONE[banner.tone];
	return (
		<View
			style={[
				styles.banner,
				{ borderColor: tint(tone.hue, 0.35), backgroundColor: tint(tone.hue, 0.08) },
			]}
			accessibilityRole="alert"
			accessibilityLiveRegion="polite"
		>
			<View style={styles.top}>
				<ConnectionDot look={tone.look} size={8} decorative />
				<Text style={[styles.title, { color: tone.text }]}>{banner.title}</Text>
			</View>
			<Text style={styles.body}>{banner.body}</Text>
		</View>
	);
};

const styles = StyleSheet.create({
	banner: {
		marginHorizontal: 10,
		marginTop: 10,
		paddingHorizontal: 12,
		paddingVertical: 10,
		gap: 3,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
	},
	top: { flexDirection: "row", alignItems: "center", gap: 8 },
	title: { fontSize: size.small, fontWeight: "600" },
	body: { color: color.mutedForeground, fontSize: size.meta },
});
