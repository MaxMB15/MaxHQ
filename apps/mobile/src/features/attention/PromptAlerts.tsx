// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// While the app is on screen, each new prompt buzzes hard, whatever screen
// is up, from the live view rather than from pushes, so it works without
// them and buzzes once. One for an agent other than the one on screen also
// drops a banner from the top that opens it. What was waiting when the app
// came back, or came in with a device as it reconnected, stays quiet.

import { useI18n } from "@maxhq/shared/i18n";

import * as Haptics from "expo-haptics";
import { useEffect, useState } from "react";
import { AppState, Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import {
	type Arrival,
	arrivalText,
	bannerArrival,
	newPrompts,
	stillWaiting,
	waitingPrompts,
} from "@maxhq/shared/push/arrivals";

import { pushNavigation, shownAgent } from "../../push/PushSetup";
import { useServices, useView } from "../../services";
import { color, radius, size, tint } from "../../theme";
import { ActivityDot } from "../../ui/Dot";

// How long the banner stays when nobody taps it.
const BANNER_MS = 8_000;
// The gap between the buzz's two pulses.
const PULSE_MS = 180;

/** Two strong pulses, harder to miss than one. */
const buzz = () => {
	void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Warning);
	setTimeout(() => void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Heavy), PULSE_MS);
};

const Banner = ({ arrival, onDone }: { arrival: Arrival; onDone: () => void }) => {
	const { t } = useI18n();

	const insets = useSafeAreaInsets();
	const open = useView((s) => stillWaiting(s.view, arrival));
	useEffect(() => {
		if (!open) onDone();
	}, [open, onDone]);
	useEffect(() => {
		const timer = setTimeout(onDone, BANNER_MS);
		// Gone once you open the agent another way.
		const moved = pushNavigation.addListener("state", () => {
			if (shownAgent() === arrival.agent.id) onDone();
		});
		return () => {
			clearTimeout(timer);
			moved();
		};
	}, [arrival, onDone]);
	const { title, body } = arrivalText(arrival);
	const show = () => {
		onDone();
		if (pushNavigation.isReady()) {
			pushNavigation.navigate("Agent", {
				agent: arrival.agent.id,
				attention: arrival.attention,
			});
		}
	};
	return (
		<View style={[styles.place, { top: insets.top + 6 }]} pointerEvents="box-none">
			<Pressable
				style={({ pressed }) => [styles.banner, pressed && styles.pressed]}
				onPress={show}
				accessibilityRole="button"
				accessibilityLabel={t("{{title}} needs you: {{body}}. Open it.", { title, body })}
				accessibilityLiveRegion="assertive"
			>
				<ActivityDot look="needs_input" surface={color.card} decorative />
				<View style={styles.text}>
					<Text style={styles.title} numberOfLines={1}>
						{title}
					</Text>
					<Text style={styles.body} numberOfLines={2}>
						{body}
					</Text>
				</View>
				<Text style={styles.open}>{t("Open")}</Text>
			</Pressable>
		</View>
	);
};

/** Mounted once beside the navigation. */
export const PromptAlerts = () => {
	const { view } = useServices();
	const [banner, setBanner] = useState<Arrival | null>(null);
	useEffect(() => {
		let known = waitingPrompts(view.getState().view);
		let active = AppState.currentState === "active";
		const look = () => {
			const now = view.getState().view;
			if (!active) {
				known = waitingPrompts(now);
				return;
			}
			const next = newPrompts(known, now, Date.now());
			known = next.known;
			if (next.arrived.length === 0) return;
			buzz();
			const shown = bannerArrival(next.arrived, shownAgent());
			if (shown) setBanner(shown);
		};
		const changed = view.subscribe(look);
		const watch = AppState.addEventListener("change", (state) => {
			active = state === "active";
			// What came while away was pushed. Only what comes from here buzzes.
			if (active) known = waitingPrompts(view.getState().view);
		});
		return () => {
			changed();
			watch.remove();
		};
	}, [view]);
	const [done] = useState(() => () => setBanner(null));
	return banner ? <Banner key={banner.attention} arrival={banner} onDone={done} /> : null;
};

const styles = StyleSheet.create({
	place: { position: "absolute", left: 10, right: 10 },
	banner: {
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingHorizontal: 12,
		paddingVertical: 10,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: tint(color.ready, 0.45),
		backgroundColor: color.card,
		shadowColor: "#000",
		shadowOpacity: 0.4,
		shadowRadius: 12,
		shadowOffset: { width: 0, height: 4 },
	},
	pressed: { backgroundColor: color.accent },
	text: { flex: 1, gap: 2 },
	title: { color: color.foregroundStrong, fontSize: size.small, fontWeight: "600" },
	body: { color: color.mutedForeground, fontSize: size.meta },
	open: { color: color.primary, fontSize: size.small, fontWeight: "600" },
});
