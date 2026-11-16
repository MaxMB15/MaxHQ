// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A file's lines on the phone, numbered, as the desktop's code view draws
// them: in blocks of lines, each one text view with its numbers beside it,
// so a long file mounts only the blocks near where you are. Every line is
// the same height, so the list knows where each block sits before it draws
// it. The whole file scrolls sideways together, as wide as its widest line.
// Colours come for a file short enough to take them, and plain text shows
// until they're ready. A line asked for sits a third of the way down,
// marked.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useEffect, useMemo, useRef, useState } from "react";
import { FlatList, ScrollView, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { colourable } from "@maxhq/shared/agent-view/code-text";
import { extensionOf, fileName } from "@maxhq/shared/agent-view/viewer-files";

import { color, font, size, tint } from "../../theme";
import { languageOf, type Lines, tokenStyle, useHighlight } from "../../ui/highlight";
import {
	BLOCK,
	type LineBlock,
	lineBlocks,
	scrollTo,
	shownLine,
	shownTokens,
	widestLine,
} from "./file";

/** Space above the first line and below the last. */
const PAD = 8;

/** Space each side of the line numbers, and after the code. */
const GUTTER_PAD = 12;

// A monospaced character's width for each point of its size, as Menlo has it.
const ADVANCE = 0.602;

// The largest text size the list follows, so a line keeps the height the list counts on.
const MOST_SCALE = 2;

/** The text size, how tall a line is, and how wide the numbers are, at the reader's text size. */
type Metrics = {
	readonly fontSize: number;
	readonly lineHeight: number;
	readonly gutter: number;
};

// One block's numbers and lines. A block draws again only when its colours
// come or its marked line changes.
const Block = memo(
	({
		block,
		marked,
		metrics,
	}: {
		block: LineBlock;
		/** The marked line, when it's in this block. */
		marked: number | null;
		metrics: Metrics;
	}) => {
		const { fontSize, lineHeight, gutter } = metrics;
		const text = { fontSize, lineHeight };
		const { first, lines, colours } = block;
		const last = lines.length - 1;
		return (
			<View style={[styles.block, { height: lines.length * lineHeight }]}>
				{marked !== null ? (
					<View
						style={[
							styles.mark,
							{ top: (marked - first) * lineHeight, height: lineHeight },
						]}
					/>
				) : null}
				<Text
					allowFontScaling={false}
					importantForAccessibility="no"
					accessibilityElementsHidden
					style={[styles.numbers, text, { width: gutter }]}
				>
					{lines.map((_, i) => String(first + i)).join("\n")}
				</Text>
				<Text allowFontScaling={false} selectable style={[styles.code, text]}>
					{colours
						? lines.map((_, i) => (
								// A line's place is its identity.
								<Text key={i}>
									{shownTokens(colours[i] ?? []).map((token, j) => (
										<Text key={j} style={tokenStyle(token)}>
											{token.content}
										</Text>
									))}
									{i < last ? "\n" : null}
								</Text>
							))
						: lines.map(shownLine).join("\n")}
				</Text>
			</View>
		);
	}
);
Block.displayName = "Block";

// The colours for the whole file once they're ready, or null. Shiki can give
// fewer lines than there are, and then none of them fit.
const useColours = (
	lines: readonly string[],
	lang: ReturnType<typeof languageOf>,
	wanted: boolean
): Lines | null => {
	const code = useMemo(() => (lang && wanted ? lines.join("\n") : ""), [lang, wanted, lines]);
	const found = useHighlight(code, lang, wanted, Number.POSITIVE_INFINITY);
	return found && found.length === lines.length ? found : null;
};

/** A file's lines, numbered, coloured when it's short enough, at `line` when it's given. */
export const CodeLines = ({
	path,
	lines,
	bytes,
	line,
	ask,
}: {
	path: string;
	lines: readonly string[];
	bytes: number;
	line: number | undefined;
	/** A new value for each ask to show the line, which scrolls to it again. */
	ask: object;
}) => {
	const { t } = useI18n();
	const { fontScale, width: screen } = useWindowDimensions();
	const insets = useSafeAreaInsets();
	const list = useRef<FlatList<LineBlock>>(null);
	// How tall the list is. A new height alone doesn't scroll again.
	const height = useRef(0);
	const [laid, setLaid] = useState(false);
	const [marked, setMarked] = useState<number | null>(null);

	const metrics = useMemo<Metrics>(() => {
		const scale = Math.min(Math.max(fontScale, 1), MOST_SCALE);
		const fontSize = size.meta * scale;
		return {
			fontSize,
			lineHeight: Math.round(18 * scale),
			gutter: Math.ceil(String(lines.length).length * fontSize * ADVANCE) + GUTTER_PAD * 2,
		};
	}, [fontScale, lines.length]);
	const lang = languageOf(extensionOf(path));
	const wanted = colourable({ lines, bytes });
	const colours = useColours(lines, lang, wanted);
	const blocks = useMemo(() => lineBlocks(lines, colours), [lines, colours]);
	const wide = useMemo(() => widestLine(lines), [lines]);
	// The least the list is wide. It grows past this for a line wider than counted.
	const width = Math.max(
		screen,
		metrics.gutter + Math.ceil(wide * metrics.fontSize * ADVANCE) + GUTTER_PAD
	);

	// Each ask to show a line scrolls it a third of the way down and marks it.
	useEffect(() => {
		if (!laid || line === undefined) return;
		const to = scrollTo(line, lines.length, {
			height: height.current,
			lineHeight: metrics.lineHeight,
			pad: PAD,
		});
		setMarked(to.line);
		list.current?.scrollToOffset({ offset: to.offset, animated: false });
	}, [ask, laid, line, lines.length, metrics.lineHeight]);

	return (
		<View style={styles.fill}>
			{lang && !wanted ? (
				<Text style={styles.note}>
					{t("This file is long, so it shows without colours.")}
				</Text>
			) : null}
			<ScrollView
				horizontal
				style={styles.fill}
				showsHorizontalScrollIndicator={false}
				onLayout={(event) => {
					height.current = event.nativeEvent.layout.height;
					setLaid(height.current > 0);
				}}
				accessibilityLabel={t("{{value1}}, read-only", { value1: fileName(path) })}
			>
				<FlatList
					ref={list}
					style={{ minWidth: width }}
					data={blocks}
					extraData={marked}
					keyExtractor={(block) => String(block.first)}
					renderItem={({ item }) => (
						<Block
							block={item}
							metrics={metrics}
							marked={
								marked !== null &&
								marked >= item.first &&
								marked < item.first + item.lines.length
									? marked
									: null
							}
						/>
					)}
					getItemLayout={(_, index) => ({
						length: (blocks[index]?.lines.length ?? BLOCK) * metrics.lineHeight,
						offset: PAD + index * BLOCK * metrics.lineHeight,
						index,
					})}
					contentContainerStyle={{ paddingTop: PAD, paddingBottom: PAD + insets.bottom }}
					initialNumToRender={2}
					maxToRenderPerBatch={2}
					windowSize={5}
				/>
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	fill: { flex: 1 },
	note: {
		paddingHorizontal: 12,
		paddingVertical: 6,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.border,
		color: color.mutedForeground,
		fontSize: size.tiny,
	},
	block: { flexDirection: "row" },
	mark: { position: "absolute", left: 0, right: 0, backgroundColor: tint(color.primary, 0.15) },
	numbers: {
		paddingHorizontal: GUTTER_PAD,
		color: color.subtleForeground,
		fontFamily: font.mono,
		textAlign: "right",
		includeFontPadding: false,
	},
	// Grows to the list's width, but never narrower than its longest line, so no line wraps.
	code: {
		flexGrow: 1,
		paddingRight: GUTTER_PAD,
		color: color.terminal,
		fontFamily: font.mono,
		includeFontPadding: false,
	},
});
