// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A drawing in the feed: its shown page in a web view at the drawing's size,
// which scrolls sideways when it's wider than the feed. The web view takes no
// touches, so the feed and the row scroll over it, and a diagram shown whole
// zooms it.

import { useMemo } from "react";
import { ScrollView, StyleSheet, View } from "react-native";
import { WebView, type WebViewProps } from "react-native-webview";

import { shownPage } from "./pages";

/** A web view's first address, which it never leaves. */
export const stays = ({ url }: { url: string }) => url.startsWith("about:");

/** The web view settings both kinds of page keep, as a visual's page does. */
export const LOCKED: WebViewProps = {
	// Any other list sends a page's own loads to the system before the check.
	originWhitelist: ["*"],
	onShouldStartLoadWithRequest: stays,
	setSupportMultipleWindows: false,
	javaScriptCanOpenWindowsAutomatically: false,
	allowFileAccess: false,
	allowsLinkPreview: false,
	dataDetectorTypes: "none",
	incognito: true,
};

type Drawn = {
	body: string;
	width: number;
	height: number;
	label: string;
};

/** A drawing's body in a web view at `width` by `height`, read out as `label`. It takes no touches. */
export const DrawnPage = ({ body, width, height, label }: Drawn) => {
	const source = useMemo(() => ({ html: shownPage(body) }), [body]);
	return (
		<View
			accessible
			accessibilityRole="image"
			accessibilityLabel={label}
			pointerEvents="none"
			style={{ width, height }}
		>
			<WebView
				{...LOCKED}
				source={source}
				javaScriptEnabled={false}
				scrollEnabled={false}
				bounces={false}
				overScrollMode="never"
				textInteractionEnabled={false}
				showsHorizontalScrollIndicator={false}
				showsVerticalScrollIndicator={false}
				style={styles.web}
			/>
		</View>
	);
};

/** A drawing's body at `width` by `height`, read out as `label`. */
export const DrawingView = ({
	inset,
	...drawn
}: Drawn & {
	/** Room around the drawing, inside the sideways scroll. */
	inset: { readonly horizontal: number; readonly vertical: number };
}) => (
	<ScrollView
		horizontal
		showsHorizontalScrollIndicator={false}
		contentContainerStyle={[
			styles.row,
			{ paddingHorizontal: inset.horizontal, paddingVertical: inset.vertical },
		]}
	>
		<DrawnPage {...drawn} />
	</ScrollView>
);

const styles = StyleSheet.create({
	row: { flexGrow: 1, justifyContent: "center" },
	web: { backgroundColor: "transparent" },
});
