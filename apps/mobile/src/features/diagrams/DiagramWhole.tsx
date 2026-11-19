// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A diagram shown whole, in a sheet over the feed. It starts fitting the
// screen, and a pinch zooms it up to its own size and past it. Close or a
// swipe down shuts it, and so does Back on Android. Android's scroll views
// don't zoom, so there it shows at the size it's laid out at, which for a
// large one is its own, and pans both ways.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { Modal, Platform, ScrollView, StyleSheet, View } from "react-native";

import { color } from "../../theme";
import { Header } from "../../ui/Header";
import type { Drawing } from "./drawings";
import { DrawnPage } from "./DrawingView";
import { wholeLayout } from "./whole";

type Size = { readonly width: number; readonly height: number };

/** A drawing that came out, as `useDrawing` gives it. */
export type DrawnDrawing = Extract<Drawing, { state: "drawn" }>;

// Room around the drawing when it fits the screen.
const INSET = 12;

const Zoomed = ({ drawing, label, box }: { drawing: DrawnDrawing; label: string; box: Size }) => {
	const layout = wholeLayout(drawing, {
		width: Math.max(1, box.width - 2 * INSET),
		height: Math.max(1, box.height - 2 * INSET),
	});
	const page = (
		<DrawnPage body={drawing.body} width={layout.width} height={layout.height} label={label} />
	);
	if (Platform.OS === "ios") {
		return (
			<ScrollView
				style={styles.fill}
				contentContainerStyle={{ width: layout.width, height: layout.height }}
				minimumZoomScale={layout.least}
				maximumZoomScale={layout.most}
				zoomScale={layout.least}
				centerContent
				showsHorizontalScrollIndicator={false}
				showsVerticalScrollIndicator={false}
			>
				{page}
			</ScrollView>
		);
	}
	return (
		<ScrollView style={styles.fill} contentContainerStyle={styles.middle} nestedScrollEnabled>
			<ScrollView horizontal contentContainerStyle={styles.middle} nestedScrollEnabled>
				<View style={styles.inset}>{page}</View>
			</ScrollView>
		</ScrollView>
	);
};

/** A diagram's drawing shown whole, read out as `label`, until `onClose`. */
export const DiagramWhole = ({
	drawing,
	label,
	onClose,
}: {
	drawing: DrawnDrawing;
	label: string;
	onClose: () => void;
}) => {
	const { t } = useI18n();
	// The room under the header, once it's laid out.
	const [box, setBox] = useState<Size | null>(null);
	return (
		<Modal visible animationType="slide" presentationStyle="pageSheet" onRequestClose={onClose}>
			<View accessibilityViewIsModal onAccessibilityEscape={onClose} style={styles.sheet}>
				<Header
					title={t("Diagram")}
					sheet
					back={{ label: t("Close"), onPress: onClose, close: true }}
				/>
				<View
					style={styles.fill}
					onLayout={(event) => {
						const { width, height } = event.nativeEvent.layout;
						setBox({ width, height });
					}}
				>
					{box ? <Zoomed drawing={drawing} label={label} box={box} /> : null}
				</View>
			</View>
		</Modal>
	);
};

const styles = StyleSheet.create({
	sheet: { flex: 1, backgroundColor: color.background },
	fill: { flex: 1 },
	middle: { flexGrow: 1, alignItems: "center", justifyContent: "center" },
	inset: { padding: INSET },
});
