// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual's page in a web view, in its card or over the whole app. The
// page is one document with the libraries the app carries in it and a
// content policy that keeps it off the network, unless the user lets a
// host through with Load this time, which lasts until the page goes. It
// never leaves its first address, and a link the user taps opens in the
// browser after the same checks as the desktop's. One on an agent's
// device comes through the media cache, as a picture does. Bars say what
// the page asked for that it can't have, and its first error, with Tell the
// agent when the card has an agent to tell.

import { useI18n } from "@maxhq/shared/i18n";

import { File } from "expo-file-system";
import { useEffect, useMemo, useReducer, useState } from "react";
import { Linking, StyleSheet, Text, View } from "react-native";
import { WebView, type WebViewMessageEvent } from "react-native-webview";

import { formatBytes } from "@maxhq/shared/agent-view/format";
import { loadMedia, useMedia } from "@maxhq/shared/agent-view/media";
import type { AgentId } from "@maxhq/shared/bindings";
import {
	HEIGHT_START,
	type HeightTrack,
	nextHeight,
	readFrameReport,
} from "@maxhq/shared/visuals/frame";
import { inlineVisualDocument, readBridgeReport } from "@maxhq/shared/visuals/inline";
import { addReport, NO_REPORTS, type VisualError, withHosts } from "@maxhq/shared/visuals/reports";
import { MAX_VISUAL_BYTES } from "@maxhq/shared/visuals/visual";

import { useServices } from "../../services";
import { color, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { Spinner } from "../../ui/Dot";
import { cardHeight, keepHeight, knownHeight } from "./card";
import { bundledLibraries } from "./libraries";
import { blockedBar, errorText, linkGate, missingText, VISUAL_VARIABLES } from "./visual";

// Only the document the view gives it, which has no address of its own.
const stays = ({ url }: { url: string }) => url.startsWith("about:");

const Bar = ({
	text,
	look,
	action,
}: {
	text: string;
	look?: "error" | "blocked";
	action?: { readonly label: string; readonly run: () => void } | null;
}) => (
	<View
		accessibilityRole={look === "error" ? "alert" : undefined}
		style={[
			styles.bar,
			look === "error" ? styles.errorBar : look === "blocked" ? styles.blockedBar : null,
		]}
	>
		<Text
			style={[
				styles.barText,
				look === "error"
					? styles.errorText
					: look === "blocked"
						? styles.blockedText
						: null,
			]}
		>
			{text}
		</Text>
		{action ? <Button small label={action.label} onPress={action.run} /> : null}
	</View>
);

/** Where a card's page draws: at most `cap` tall, its height kept under `mark`. */
export type InCard = { readonly cap: number; readonly mark: string };

/** Puts the page's errors in the box of the agent that showed it. */
export type Tell = (errors: readonly VisualError[], count: number) => void;

// One run of the page. New hosts or a Reload make a new one.
const Run = ({
	html,
	card,
	hosts,
	onLoad,
	onTell,
}: {
	html: string;
	card: InCard | undefined;
	hosts: readonly string[];
	onLoad: (hosts: readonly string[]) => void;
	onTell: Tell | undefined;
}) => {
	const { t } = useI18n();
	const { document, missing } = useMemo(
		() =>
			inlineVisualDocument(
				{ html, theme: "dark", variables: VISUAL_VARIABLES },
				bundledLibraries(),
				hosts
			),
		[html, hosts]
	);
	const source = useMemo(() => ({ html: document }), [document]);
	const [reports, report] = useReducer(addReport, NO_REPORTS);
	// Null until the page says how tall it is.
	const [track, setTrack] = useState<HeightTrack | null>(null);
	const [gate] = useState(linkGate);

	const mark = card?.mark ?? null;
	useEffect(() => {
		if (mark !== null && track) keepHeight(mark, track.height);
	}, [mark, track]);

	const onMessage = (event: WebViewMessageEvent) => {
		const said = readFrameReport(readBridgeReport(event.nativeEvent.data));
		switch (said?.type) {
			case "maxhq-visual:height":
				if (card) setTrack((was) => nextHeight(was ?? HEIGHT_START, said.height));
				return;
			case "maxhq-visual:error":
				report({ type: "error", error: { message: said.message, line: said.line } });
				return;
			case "maxhq-visual:blocked":
				report({ type: "blocked", url: said.url, what: said.what });
				return;
			case "maxhq-visual:link":
				if (gate(said.url, Date.now())) Linking.openURL(said.url).catch(() => {});
		}
	};

	const [first] = reports.errors;
	const blocked = blockedBar(reports.blocked, hosts);
	const absent = missingText(missing);
	const sized = card ? cardHeight(track?.height ?? knownHeight(card.mark), card.cap) : null;
	return (
		<>
			{first ? (
				<Bar
					look="error"
					text={errorText(first, reports.errorCount)}
					action={
						onTell
							? {
									label: t("Tell the agent"),
									run: () => onTell(reports.errors, reports.errorCount),
								}
							: null
					}
				/>
			) : null}
			{blocked ? (
				<Bar
					look="blocked"
					text={blocked.text}
					action={
						blocked.load.length > 0
							? { label: t("Load this time"), run: () => onLoad(blocked.load) }
							: null
					}
				/>
			) : null}
			{absent ? <Bar text={absent} /> : null}
			<WebView
				source={source}
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
				// In a card, the page scrolls only past the card's cap, and the feed takes other drags.
				scrollEnabled={!sized || sized.scrolls}
				nestedScrollEnabled={sized?.scrolls ?? false}
				bounces={!sized}
				overScrollMode={sized ? "never" : "always"}
				style={styles.web}
				containerStyle={sized ? { height: sized.height } : styles.fill}
			/>
		</>
	);
};

/**
 * A visual's page with its bars: in a card when `card` says where, else
 * filling its place. `runs` counts Reload, each of which runs it again.
 * The error bar offers Tell the agent only when given `onTell`.
 */
export const VisualPage = ({
	html,
	card,
	runs = 0,
	onTell,
}: {
	html: string;
	card?: InCard;
	runs?: number;
	onTell?: Tell;
}) => {
	// The hosts Load this time let through, which a Reload keeps.
	const [hosts, setHosts] = useState<readonly string[]>([]);
	return (
		<Run
			key={`${hosts.join(",")}\n${runs}`}
			html={html}
			card={card}
			hosts={hosts}
			onLoad={(more) => setHosts((was) => withHosts(was, more))}
			onTell={onTell}
		/>
	);
};

/** Why a visual's page didn't come, and how to ask again when that can help. */
export type NoPageWhy = { readonly failure: string; readonly retry: (() => void) | null };

/** A visual's page read from an agent's device, why it couldn't be, or null while it comes. */
export type DeviceHtml = { readonly html: string } | NoPageWhy | null;

/** Fetches a visual on an agent's device into the media cache, and reads it. */
export const useDeviceHtml = (agent: AgentId, path: string): DeviceHtml => {
	const { t } = useI18n();
	const { source } = useServices();
	const media = useMedia(agent, path);
	const [page, setPage] = useState<
		{ url: string; html: string } | { url: string; failed: true } | null
	>(null);
	// Counts Try again after the file came but couldn't be read.
	const [reads, setReads] = useState(0);
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
	}, [url, reads]);

	if (page && page.url === url) {
		return "html" in page
			? page
			: { failure: t("Couldn't read the visual."), retry: () => setReads((n) => n + 1) };
	}
	const load = () => void loadMedia(source, agent, path, MAX_VISUAL_BYTES);
	switch (media?.state) {
		case "failed":
			return { failure: media.reason, retry: load };
		case "large":
			return {
				failure: t("The visual is {{size}}, too large to show.", {
					size: formatBytes(media.size),
				}),
				retry: null,
			};
		default:
			return null;
	}
};

/** What shows in a page's place while it comes, or why it didn't: `height` tall, else filling it. */
export const NoPage = ({ why, height }: { why: NoPageWhy | null; height?: number }) => {
	const { t } = useI18n();
	return (
		<View style={[styles.note, height === undefined ? styles.fill : { height }]}>
			{why ? (
				<>
					<Text style={styles.noteText}>{why.failure}</Text>
					{why.retry ? <Button small label={t("Try again")} onPress={why.retry} /> : null}
				</>
			) : (
				<Spinner />
			)}
		</View>
	);
};

const styles = StyleSheet.create({
	bar: {
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingHorizontal: 12,
		paddingVertical: 6,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.border,
		backgroundColor: color.subpanel,
	},
	barText: { flex: 1, color: color.mutedForeground, fontSize: size.meta },
	errorBar: { backgroundColor: tint(color.error, 0.1) },
	errorText: { color: color.errorText },
	blockedBar: { backgroundColor: tint(color.busy, 0.1) },
	blockedText: { color: color.busyText },
	web: { backgroundColor: color.background },
	fill: { flex: 1 },
	note: { alignItems: "center", justifyContent: "center", gap: 10, padding: 24 },
	noteText: { color: color.mutedForeground, fontSize: size.small, textAlign: "center" },
});
