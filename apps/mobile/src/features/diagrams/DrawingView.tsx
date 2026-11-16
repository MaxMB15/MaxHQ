// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A drawing in the feed: its shown page in a web view at the drawing's size,
// which scrolls sideways when it's wider than the feed. The web view takes no
// touches, so the feed and the row scroll over it.

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

/** A drawing's body at `width` by `height`, read out as `label`. */
export const DrawingView = ({
	body,
	width,
	height,
	label,
	inset,
}: {
	body: string;
	width: number;
	height: number;
	label: string;
	/** Room around the drawing, inside the sideways scroll. */
	inset: { readonly horizontal: number; readonly vertical: number };
}) => {
	const source = useMemo(() => ({ html: shownPage(body) }), [body]);
	return (
		<ScrollView
			horizontal
			showsHorizontalScrollIndicator={false}
			contentContainerStyle={[
				styles.row,
				{ paddingHorizontal: inset.horizontal, paddingVertical: inset.vertical },
			]}
		>
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
		</ScrollView>
	);
};

const styles = StyleSheet.create({
	row: { flexGrow: 1, justifyContent: "center" },
	web: { backgroundColor: "transparent" },
});
