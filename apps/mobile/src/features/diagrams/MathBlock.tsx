// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A formula on lines of its own, drawn by KaTeX on a hidden page (see
// DiagramRenderer.tsx) and centred, as on the desktop. Its source stands for
// it while the text is still coming in, until it's drawn, and if it can't
// be. It follows the system's text size.

import { StyleSheet, Text, useWindowDimensions } from "react-native";

import { color, font, size } from "../../theme";
import { useDrawing } from "./drawings";
import { DrawingView } from "./DrawingView";

// KaTeX's formulas are this much larger than the text around them.
const MATH_SCALE = 1.21;

const INSET = { horizontal: 0, vertical: 4 } as const;

/** A display formula, as `$$` or a fenced `math` block gives it. `live` is set while it's still streaming in. */
export const MathBlock = ({ tex, live = false }: { tex: string; live?: boolean }) => {
	const { fontScale } = useWindowDimensions();
	const source = tex.trim();
	const fontSize = Math.round(size.body * MATH_SCALE * fontScale);
	const wanted = !live && source !== "";
	const drawing = useDrawing("math", source, fontSize, wanted);
	if (source === "") return null;
	if (!wanted || drawing?.state !== "drawn")
		return (
			<Text selectable style={styles.source}>
				{source}
			</Text>
		);
	return (
		<DrawingView
			body={drawing.body}
			width={drawing.width}
			height={drawing.height}
			label={source}
			inset={INSET}
		/>
	);
};

const styles = StyleSheet.create({
	source: {
		color: color.mutedForeground,
		fontFamily: font.mono,
		fontSize: size.tiny,
		lineHeight: 18,
	},
});
