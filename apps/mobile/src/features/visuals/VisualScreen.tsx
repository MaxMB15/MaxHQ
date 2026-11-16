// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual shown whole, over the whole app, in a web view of its own, with
// the bars and Load this time its card has. One on an agent's device is
// fetched once the screen opens.

import { useI18n } from "@maxhq/shared/i18n";

import { Modal, Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useStore } from "zustand";

import type { AgentId } from "@maxhq/shared/bindings";

import { color, size } from "../../theme";
import { Close } from "../../ui/Icon";
import { closeVisual, shownVisual, visualName } from "./visual";
import { NoPage, useDeviceHtml, VisualPage } from "./VisualPage";

// A visual on an agent's device, once it's read.
const DevicePage = ({ agent, path }: { agent: AgentId; path: string }) => {
	const page = useDeviceHtml(agent, path);
	return page && "html" in page ? <VisualPage html={page.html} /> : <NoPage why={page} />;
};

/** The visual shown whole, over the whole app. Mount it once. */
export const VisualScreen = () => {
	const { t } = useI18n();

	const visual = useStore(shownVisual, (s) => s.visual);
	const insets = useSafeAreaInsets();
	if (!visual) return null;
	const name = visualName(visual.title);
	return (
		<Modal visible animationType="slide" onRequestClose={closeVisual}>
			<View
				accessibilityViewIsModal
				onAccessibilityEscape={closeVisual}
				style={[styles.screen, { paddingTop: insets.top, paddingBottom: insets.bottom }]}
			>
				<View style={styles.header}>
					<Text style={styles.diamond}>◆</Text>
					<Text accessibilityRole="header" numberOfLines={1} style={styles.title}>
						{name}
					</Text>
					<Pressable
						accessibilityRole="button"
						accessibilityLabel={t("Close")}
						onPress={closeVisual}
						hitSlop={8}
						style={styles.close}
					>
						<Close tint={color.foregroundStrong} />
					</Pressable>
				</View>
				{visual.kind === "page" ? (
					<VisualPage html={visual.html} />
				) : (
					<DevicePage
						key={`${visual.agent}\n${visual.path}`}
						agent={visual.agent}
						path={visual.path}
					/>
				)}
			</View>
		</Modal>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	header: {
		minHeight: 48,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingLeft: 16,
		paddingRight: 8,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.border,
	},
	diamond: { color: color.visual, fontSize: size.body },
	title: { flex: 1, color: color.foregroundStrong, fontSize: size.heading, fontWeight: "600" },
	close: { width: 36, height: 36, alignItems: "center", justifyContent: "center" },
});
