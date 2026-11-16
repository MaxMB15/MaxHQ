// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The strips over the agent list about updates: a newer MaxHQ in
// TestFlight, which you open or dismiss for that version, and each device
// whose maxd speaks only protocols newer than this app's. The phone looks
// for the newest build from here, when the app opens and comes back.

import { useI18n } from "@maxhq/shared/i18n";

import { type ReactNode, useEffect, useMemo } from "react";
import { StyleSheet, Text, View } from "react-native";
import { useStore } from "zustand";

import { PROTOCOL_VERSION } from "@maxhq/shared/bindings";

import { useAppActive, useView } from "../../services";
import { color, radius, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { checkIfDue, dismiss, openTestflight, thisBuild, updates } from "./store";
import { appTooOld, offeredVersion } from "./updates";

const Banner = ({
	hue,
	text,
	title,
	body,
	children,
}: {
	hue: string;
	text: string;
	title: string;
	body: string;
	children?: ReactNode;
}) => (
	<View
		style={[styles.banner, { borderColor: tint(hue, 0.35), backgroundColor: tint(hue, 0.08) }]}
		accessibilityRole="alert"
	>
		<View style={styles.words}>
			<Text style={[styles.title, { color: text }]}>{title}</Text>
			<Text style={styles.body}>{body}</Text>
		</View>
		{children}
	</View>
);

export const UpdateBanners = () => {
	const { t } = useI18n();

	const offered = useStore(updates, (state) => offeredVersion(state, thisBuild.version));
	const view = useView((s) => s.view);
	const tooNew = useMemo(() => appTooOld(view), [view]);
	const active = useAppActive();
	useEffect(() => {
		if (active) checkIfDue();
	}, [active]);

	return (
		<>
			{offered ? (
				<Banner
					hue={color.primary}
					text={color.foregroundStrong}
					title={t("MaxHQ {{version}} is ready", { version: offered })}
					body={t("Update in TestFlight")}
				>
					<Button label={t("Open")} small onPress={() => void openTestflight()} />
					<Button
						label={t("Dismiss")}
						tone="link"
						small
						onPress={() => dismiss(offered)}
					/>
				</Banner>
			) : null}
			{tooNew.map((device) => (
				<Banner
					key={device.link}
					hue={color.connecting}
					text={color.connectingText}
					title={t("{{name}} needs a newer app", { name: device.name })}
					body={t("Its maxd speaks protocol {{theirs}}; this app speaks {{ours}}", {
						theirs: device.theirs,
						ours: PROTOCOL_VERSION,
					})}
				/>
			))}
		</>
	);
};

const styles = StyleSheet.create({
	banner: {
		marginHorizontal: 10,
		marginTop: 10,
		paddingHorizontal: 12,
		paddingVertical: 10,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
	},
	words: { flex: 1, gap: 3 },
	title: { fontSize: size.small, fontWeight: "600" },
	body: { color: color.mutedForeground, fontSize: size.meta },
});
