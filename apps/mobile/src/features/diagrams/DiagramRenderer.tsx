// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The hidden pages that draw the feed's diagrams and formulas, mounted once
// for the whole app. Each comes the first time a block asks for its kind,
// and stays. It draws one thing at a time, and one that takes too long, or
// a page whose process stops, fails that drawing and starts the page again.

import { t } from "@maxhq/shared/i18n";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { StyleSheet, View } from "react-native";
import { WebView, type WebViewMessageEvent } from "react-native-webview";
import { useStore } from "zustand";

import { bundledLibraries } from "../visuals/libraries";
import { LOCKED } from "./DrawingView";
import { type DrawKind, drawings } from "./drawings";
import { drawingOf, readAnswer, rendererPage } from "./pages";

// Mermaid has no limit of its own on how long it draws.
const TOO_LONG_MS = 15_000;

const RendererPage = ({ kind }: { kind: DrawKind }) => {
	const source = useMemo(() => ({ html: rendererPage(kind, bundledLibraries()) }), [kind]);
	// Counts restarts, each of which makes the page again.
	const [round, setRound] = useState(0);
	// Its type takes the props it adds, which here are none.
	const view = useRef<WebView<object>>(null);
	const ready = useRef(false);
	const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

	const stopTimer = useCallback(() => {
		if (timer.current !== null) clearTimeout(timer.current);
		timer.current = null;
	}, []);

	const restart = useCallback(
		(reason: string) => {
			stopTimer();
			ready.current = false;
			drawings.giveUp(kind, reason);
			setRound((n) => n + 1);
		},
		[kind, stopTimer]
	);

	// Sends the next job once the page is ready and free.
	const pump = useCallback(() => {
		if (!ready.current) return;
		const job = drawings.take(kind);
		if (!job) return;
		view.current?.injectJavaScript(`window.maxhqDraw(${JSON.stringify(job)});true;`);
		timer.current = setTimeout(() => restart(t("It took too long to draw.")), TOO_LONG_MS);
	}, [kind, restart]);

	useEffect(() => drawings.store.subscribe(pump), [pump]);
	useEffect(() => stopTimer, [stopTimer]);

	const onMessage = ({ nativeEvent }: WebViewMessageEvent) => {
		const answer = readAnswer(nativeEvent.data);
		if (!answer) return;
		if (answer.type === "ready") {
			ready.current = true;
			pump();
			return;
		}
		const job = drawings.store.getState().drawing[kind];
		if (job?.key !== answer.key) return;
		stopTimer();
		// Keeping it hears the store change, which sends the next.
		drawings.settle(kind, job.key, drawingOf(answer, job));
	};

	const stopped = () => restart(t("The drawing stopped before it finished."));

	return (
		<WebView
			key={round}
			ref={view}
			{...LOCKED}
			source={source}
			onMessage={onMessage}
			onContentProcessDidTerminate={stopped}
			onRenderProcessGone={stopped}
			scrollEnabled={false}
			style={styles.web}
		/>
	);
};

/** The pages that draw for the feed. Mount it once, beside the app's screens. */
export const DiagramRenderer = () => {
	const pages = useStore(drawings.store, (state) => state.pages);
	if (pages.length === 0) return null;
	return (
		<View
			pointerEvents="none"
			accessibilityElementsHidden
			importantForAccessibility="no-hide-descendants"
			style={styles.hidden}
		>
			{pages.map((kind) => (
				<RendererPage key={kind} kind={kind} />
			))}
		</View>
	);
};

const styles = StyleSheet.create({
	// Laid out, since Mermaid measures its text, but too small and faint to see.
	hidden: { position: "absolute", left: 0, top: 0, width: 1, height: 1, opacity: 0 },
	web: { position: "absolute", left: 0, top: 0, width: 1, height: 1 },
});
