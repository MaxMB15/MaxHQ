// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A Mermaid diagram in the feed, as on the desktop: its source while the
// text is still coming in and while it draws, then the picture with its
// source a tap away. One that won't draw shows its source and why. The
// drawing happens on a hidden page (see DiagramRenderer.tsx).

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import { color, size } from "../../theme";
import { CodeCard, CodeText, CopyButton, cardStyles, HeaderButton } from "../../ui/CodeCard";
import { Spinner } from "../../ui/Dot";
import { type Drawing, useDrawing } from "./drawings";
import { DrawingView } from "./DrawingView";

/** The tallest a diagram shows in the feed, as on the desktop. */
const TALLEST = 560;

const INSET = { horizontal: 12, vertical: 12 } as const;

// At its own size, or smaller to stay under the tallest.
const Picture = ({
	drawing,
	source,
}: {
	drawing: Extract<Drawing, { state: "drawn" }>;
	source: string;
}) => {
	const scale = Math.min(1, TALLEST / drawing.height);
	return (
		<DrawingView
			body={drawing.body}
			width={Math.round(drawing.width * scale)}
			height={Math.round(drawing.height * scale)}
			label={source}
			inset={INSET}
		/>
	);
};

/** A fenced `mermaid` block. `live` is set while the block is still streaming in. */
export const Diagram = ({ source, live = false }: { source: string; live?: boolean }) => {
	const { t } = useI18n();
	const text = source.replace(/\n$/, "");
	const wanted = !live && text.trim() !== "";
	const drawing = useDrawing("diagram", text, 0, wanted);
	const [showSource, setShowSource] = useState(false);
	const drawn = wanted && drawing?.state === "drawn" ? drawing : null;
	const failed = wanted && drawing?.state === "failed" ? drawing : null;
	const buttons = (
		<>
			{wanted && !drawing ? (
				<View
					accessible
					accessibilityRole="progressbar"
					accessibilityLabel={t("Drawing the diagram")}
					style={cardStyles.button}
				>
					<Spinner tone={color.mutedForeground} size={11} width={1.5} />
				</View>
			) : null}
			{drawn ? (
				<HeaderButton
					accessibilityRole="togglebutton"
					accessibilityState={{ checked: showSource }}
					onPress={() => setShowSource((shown) => !shown)}
				>
					<Text style={cardStyles.buttonText}>
						{showSource ? t("Diagram") : t("Source")}
					</Text>
				</HeaderButton>
			) : null}
			{live ? null : <CopyButton text={text} />}
		</>
	);
	return (
		<View style={styles.holder}>
			<CodeCard name="mermaid" buttons={buttons}>
				{drawn && !showSource ? (
					<Picture drawing={drawn} source={text} />
				) : (
					<CodeText>{text}</CodeText>
				)}
			</CodeCard>
			{failed ? (
				<Text style={styles.failure}>
					{t("Couldn't draw this diagram. ")}
					{failed.reason}
				</Text>
			) : null}
		</View>
	);
};

const styles = StyleSheet.create({
	holder: { gap: 4 },
	failure: { color: color.mutedForeground, fontSize: size.tiny },
});
