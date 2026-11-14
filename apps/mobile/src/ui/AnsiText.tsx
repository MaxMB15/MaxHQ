// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A tool's output in a fixed-width font, its colour codes drawn as styled
// runs of text.

import { useMemo } from "react";
import { StyleSheet, Text, type TextStyle, View } from "react-native";

import { type AnsiStyle, ansiBlocks, parseAnsi } from "@maxhq/shared/timeline/ansi";

import { color, font, size } from "../theme";

// Lines per text block. One block for all of a long output is a layer too tall to draw.
const BLOCK = 100;

const styleOf = (style: AnsiStyle): TextStyle => ({
	color: style.fg ?? undefined,
	backgroundColor: style.bg ?? undefined,
	fontWeight: style.bold ? "700" : undefined,
	opacity: style.dim ? 0.6 : undefined,
	fontStyle: style.italic ? "italic" : undefined,
	textDecorationLine: style.underline ? "underline" : undefined,
});

export const AnsiText = ({ output }: { output: string }) => {
	const blocks = useMemo(() => ansiBlocks(parseAnsi(output.replace(/\n$/, "")), BLOCK), [output]);
	return (
		<View>
			{blocks.map((spans, b) => (
				// Blocks and runs have no identity but their place in the output.
				<Text key={b} selectable style={styles.output}>
					{spans.map((span, i) => (
						<Text key={i} style={styleOf(span.style)}>
							{span.text}
						</Text>
					))}
				</Text>
			))}
		</View>
	);
};

const styles = StyleSheet.create({
	output: {
		color: color.secondaryForeground,
		fontFamily: font.mono,
		fontSize: size.tiny,
		lineHeight: 19,
	},
});
