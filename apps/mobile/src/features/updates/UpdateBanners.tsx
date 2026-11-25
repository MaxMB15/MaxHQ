// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The strips over the agent list about updates: a newer MaxHQ in
// TestFlight, or to download on Android, which you open or dismiss for that
// version, and each device whose maxd shares no protocol with this app. When
// the app is the older side, its strip opens the update; when the device
// is, it says to update MaxHQ there. The phone looks for the newest build from here, when the app
// opens and comes back.

import { useI18n } from "@maxhq/shared/i18n";

import { type ReactNode, useEffect, useMemo } from "react";
import { StyleSheet, Text, View } from "react-native";
import { useStore } from "zustand";

import { PROTOCOL_VERSION } from "@maxhq/shared/bindings";

import { platform } from "../../platform";
import { useAppActive, useView } from "../../services";
import { color, radius, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { checkIfDue, dismiss, openUpdate, thisBuild, updates } from "./store";
import { mismatches, offeredVersion, storeNote } from "./updates";

const Banner = ({
	hue,
	text,
	title,
	body,
	note,
	children,
}: {
	hue: string;
	text: string;
	title: string;
	body: string;
	/** A line under the body, such as what to do about it. */
	note?: string;
	children?: ReactNode;
}) => (
	<View
		style={[styles.banner, { borderColor: tint(hue, 0.35), backgroundColor: tint(hue, 0.08) }]}
		accessibilityRole="alert"
	>
		<View style={styles.words}>
			<Text style={[styles.title, { color: text }]}>{title}</Text>
			<Text style={styles.body}>{body}</Text>
			{note ? <Text style={styles.body}>{note}</Text> : null}
		</View>
		{children}
	</View>
);

export const UpdateBanners = () => {
	const { t } = useI18n();

	const offered = useStore(updates, (state) =>
		offeredVersion(state, thisBuild, platform.updatesThrough)
	);
	const view = useView((s) => s.view);
	const mismatched = useMemo(() => mismatches(view), [view]);
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
					note={storeNote(platform.updatesThrough) ?? undefined}
				>
					<Button label={t("Open")} small onPress={() => void openUpdate()} />
					<Button
						label={t("Dismiss")}
						tone="link"
						small
						onPress={() => dismiss(offered)}
					/>
				</Banner>
			) : null}
			{mismatched.map((device) => {
				const body = t("Its maxd speaks protocol {{theirs}}; this app speaks {{ours}}", {
					theirs: device.theirs,
					ours: PROTOCOL_VERSION,
				});
				return device.older === "app" ? (
					<Banner
						key={device.link}
						hue={color.connecting}
						text={color.connectingText}
						title={t("{{name}} needs a newer app", { name: device.name })}
						body={body}
					>
						<Button
							label={t("Open TestFlight")}
							small
							onPress={() => void openUpdate()}
						/>
					</Banner>
				) : (
					<Banner
						key={device.link}
						hue={color.connecting}
						text={color.connectingText}
						title={t("{{name}} runs an older MaxHQ", { name: device.name })}
						body={body}
						note={t("Update MaxHQ on {{name}}.", { name: device.name })}
					/>
				);
			})}
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
