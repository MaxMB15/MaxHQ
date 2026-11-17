// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A forwarded port's page, in a sheet over the app. It opens here rather
// than in Safari, because iOS soon pauses an app in the background and the
// forward with it, so the page would stall. The page keeps to its forward.
// A link off it opens in the browser, and Reload loads it again.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import RotateCw from "lucide-react-native/icons/rotate-cw";
import { useRef, useState } from "react";
import { Linking, Modal, StyleSheet, Text, View } from "react-native";
import { WebView } from "react-native-webview";

import { color, size } from "../../theme";
import { Header, IconButton } from "../../ui/Header";
import { onForward, opensOutside, pageUrl } from "./running";

/** The page on the phone's forward at `local`, open while `local` isn't null. */
export const PortPage = ({ local, onClose }: { local: number | null; onClose: () => void }) => {
	const { t } = useI18n();

	// The props it adds to the WebView's own, which are none.
	const web = useRef<WebView<object>>(null);
	const [failure, setFailure] = useState<string | null>(null);
	const number = local === null ? "" : formatNumber(local, { useGrouping: false });

	const stays = (request: { readonly url: string; readonly isTopFrame?: boolean }) => {
		if (local === null) return false;
		// A frame inside the page loads what the page asks for.
		if (request.isTopFrame === false || onForward(request.url, local)) return true;
		if (opensOutside(request.url)) Linking.openURL(request.url).catch(() => {});
		return false;
	};

	return (
		<Modal
			visible={local !== null}
			animationType="slide"
			presentationStyle="pageSheet"
			onRequestClose={onClose}
		>
			<View style={styles.sheet}>
				<Header
					title={`localhost:${number}`}
					sheet
					back={{ label: t("Close"), onPress: onClose, close: true }}
					right={
						<IconButton
							label={t("Reload")}
							onPress={() => {
								setFailure(null);
								web.current?.reload();
							}}
						>
							<RotateCw size={17} strokeWidth={2} color={color.secondaryForeground} />
						</IconButton>
					}
				/>
				{failure !== null ? (
					<Text accessibilityRole="alert" style={styles.failure}>
						{t("Couldn't open localhost:{{local}}.{{value2}}", {
							local: number,
							value2: ` ${failure}`,
						})}
					</Text>
				) : null}
				{local !== null ? (
					<WebView
						ref={web}
						source={{ uri: pageUrl(local) }}
						originWhitelist={["*"]}
						onShouldStartLoadWithRequest={stays}
						onLoadStart={() => setFailure(null)}
						onError={(event: { nativeEvent: { description: string } }) =>
							setFailure(event.nativeEvent.description)
						}
						setSupportMultipleWindows={false}
						javaScriptCanOpenWindowsAutomatically={false}
						allowFileAccess={false}
						allowsLinkPreview={false}
						dataDetectorTypes="none"
						allowsBackForwardNavigationGestures
						style={styles.web}
						containerStyle={styles.fill}
					/>
				) : null}
			</View>
		</Modal>
	);
};

const styles = StyleSheet.create({
	sheet: { flex: 1, backgroundColor: color.background },
	fill: { flex: 1 },
	web: { flex: 1, backgroundColor: color.background },
	failure: {
		color: color.errorText,
		fontSize: size.small,
		paddingHorizontal: 16,
		paddingVertical: 10,
		borderBottomColor: color.border,
		borderBottomWidth: StyleSheet.hairlineWidth,
	},
});
