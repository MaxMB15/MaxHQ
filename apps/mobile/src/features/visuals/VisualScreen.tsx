// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual shown whole, over the whole app, in a web view of its own. The
// page is one document with the libraries the app carries in it and a
// content policy that keeps it off the network, so it loads nothing and
// never leaves its first address. A link the user taps opens in the browser
// after the same checks as the desktop's. One on an agent's device comes
// through the media cache, as a picture does. Bars say what the page asked
// for that it can't have, and its first error.

import { useI18n } from "@maxhq/shared/i18n";

import { File } from "expo-file-system";
import { useEffect, useMemo, useRef, useState } from "react";
import { Linking, Modal, Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { WebView, type WebViewMessageEvent } from "react-native-webview";
import { useStore } from "zustand";

import { formatBytes } from "@maxhq/shared/agent-view/format";
import { loadMedia, useMedia } from "@maxhq/shared/agent-view/media";
import type { AgentId } from "@maxhq/shared/bindings";
import { readFrameReport } from "@maxhq/shared/visuals/frame";
import { inlineVisualDocument, readBridgeReport } from "@maxhq/shared/visuals/inline";
import { MAX_VISUAL_BYTES } from "@maxhq/shared/visuals/visual";

import { useServices } from "../../services";
import { color, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { Spinner } from "../../ui/Dot";
import { Close } from "../../ui/Icon";
import { bundledLibraries } from "./libraries";
import {
	blockedHosts,
	blockedText,
	closeVisual,
	errorText,
	linkGate,
	missingText,
	type PageError,
	shownVisual,
	VISUAL_VARIABLES,
	visualName,
} from "./visual";

// Only the document the screen gives it, which has no address of its own.
const stays = ({ url }: { url: string }) => url.startsWith("about:");

const Bar = ({ text, error }: { text: string; error?: boolean }) => (
	<View
		accessibilityRole={error ? "alert" : undefined}
		style={[styles.bar, error ? styles.errorBar : null]}
	>
		<Text style={[styles.barText, error ? styles.errorText : null]}>{text}</Text>
	</View>
);

// The page itself, and the bars over it.
const Page = ({ html }: { html: string }) => {
	useI18n();
	const { document, missing } = useMemo(
		() =>
			inlineVisualDocument(
				{ html, theme: "dark", variables: VISUAL_VARIABLES },
				bundledLibraries()
			),
		[html]
	);
	const [errors, setErrors] = useState<{ first: PageError; count: number } | null>(null);
	const [hosts, setHosts] = useState<readonly string[]>([]);
	const gate = useRef(linkGate()).current;

	const onMessage = (event: WebViewMessageEvent) => {
		const said = readFrameReport(readBridgeReport(event.nativeEvent.data));
		switch (said?.type) {
			case "maxhq-visual:error":
				setErrors((was) => ({ first: was?.first ?? said, count: (was?.count ?? 0) + 1 }));
				return;
			case "maxhq-visual:blocked":
				setHosts((was) => blockedHosts(was, said));
				return;
			case "maxhq-visual:link":
				if (gate(said.url, Date.now())) Linking.openURL(said.url).catch(() => {});
		}
	};

	const absent = missingText(missing);
	const blocked = blockedText(hosts);
	return (
		<>
			{errors ? <Bar error text={errorText(errors.first, errors.count)} /> : null}
			{blocked ? <Bar text={blocked} /> : null}
			{absent ? <Bar text={absent} /> : null}
			<WebView
				source={{ html: document }}
				// Any other list sends a page's own loads to the system before the check below.
				originWhitelist={["*"]}
				onShouldStartLoadWithRequest={stays}
				onMessage={onMessage}
				setSupportMultipleWindows={false}
				javaScriptCanOpenWindowsAutomatically={false}
				allowFileAccess={false}
				allowsLinkPreview={false}
				dataDetectorTypes="none"
				incognito
				style={styles.web}
				containerStyle={styles.web}
			/>
		</>
	);
};

const Note = ({ text, onRetry }: { text: string; onRetry?: () => void }) => {
	const { t } = useI18n();
	return (
		<View style={styles.note}>
			<Text style={styles.noteText}>{text}</Text>
			{onRetry ? <Button small label={t("Try again")} onPress={onRetry} /> : null}
		</View>
	);
};

// A visual on an agent's device, fetched once the screen opens.
const DevicePage = ({ agent, path }: { agent: AgentId; path: string }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const media = useMedia(agent, path);
	const [page, setPage] = useState<
		{ url: string; html: string } | { url: string; failed: true } | null
	>(null);
	const load = () => void loadMedia(source, agent, path, MAX_VISUAL_BYTES);
	useEffect(() => {
		void loadMedia(source, agent, path, MAX_VISUAL_BYTES);
	}, [source, agent, path]);
	const url = media?.state === "ready" ? media.url : null;
	useEffect(() => {
		if (!url) return;
		let live = true;
		new File(url)
			.text()
			.then((html) => live && setPage({ url, html }))
			.catch(() => live && setPage({ url, failed: true }));
		return () => {
			live = false;
		};
	}, [url]);

	if (page && page.url === url) {
		return "html" in page ? (
			<Page html={page.html} />
		) : (
			<Note text={t("Couldn't read the visual.")} onRetry={load} />
		);
	}
	switch (media?.state) {
		case "failed":
			return <Note text={media.reason} onRetry={load} />;
		case "large":
			return (
				<Note
					text={t("The visual is {{size}}, too large to show.", {
						size: formatBytes(media.size),
					})}
				/>
			);
		default:
			return (
				<View style={styles.note}>
					<Spinner />
				</View>
			);
	}
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
					<Page html={visual.html} />
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
	bar: {
		paddingHorizontal: 12,
		paddingVertical: 6,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.border,
		backgroundColor: color.subpanel,
	},
	barText: { color: color.mutedForeground, fontSize: size.meta },
	errorBar: { backgroundColor: tint(color.error, 0.1) },
	errorText: { color: color.errorText },
	web: { flex: 1, backgroundColor: color.background },
	note: { flex: 1, alignItems: "center", justifyContent: "center", gap: 10, padding: 24 },
	noteText: { color: color.mutedForeground, fontSize: size.small, textAlign: "center" },
});
