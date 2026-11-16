// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { StyleSheet, Text, View } from "react-native";

import { movedBanner } from "@maxhq/shared/agent-view/move-agent";
import type { Agent } from "@maxhq/shared/bindings";
import { useI18n } from "@maxhq/shared/i18n";

import type { Screens } from "../../navigation";
import { useView } from "../../services";
import { color, radius, size } from "../../theme";
import { Button } from "../../ui/Button";

/**
 * For an agent that moved to or from another device, as the desktop has
 * it: over the composer of the old one, opening the new one, and at the
 * top of the new one's feed, opening the old one.
 */
export const MovedBanner = ({ agent, way }: { agent: Agent; way: "to" | "from" }) => {
	const { t } = useI18n();
	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const view = useView((s) => s.view);
	const banner = movedBanner(view, agent);
	if (banner?.way !== way) return null;
	const { other } = banner;
	const box = (
		<View style={styles.box} accessibilityLabel={banner.title}>
			<Text style={styles.title}>{banner.title}</Text>
			<Text style={styles.body}>{banner.detail}</Text>
			{other ? (
				<Button
					label={way === "to" ? t("Open") : t("Open the old agent")}
					tone={way === "to" ? "primary" : "secondary"}
					small
					onPress={() => navigation.navigate("Agent", { agent: other })}
				/>
			) : null}
		</View>
	);
	return way === "to" ? <View style={styles.bar}>{box}</View> : box;
};

const styles = StyleSheet.create({
	bar: { backgroundColor: color.background, paddingHorizontal: 10, paddingVertical: 8 },
	box: {
		borderColor: color.input,
		borderWidth: StyleSheet.hairlineWidth,
		borderRadius: radius.lg,
		backgroundColor: color.card,
		padding: 12,
		gap: 6,
	},
	title: { color: color.foregroundStrong, fontSize: size.small, fontWeight: "600" },
	body: { color: color.mutedForeground, fontSize: size.meta, lineHeight: 17 },
});
