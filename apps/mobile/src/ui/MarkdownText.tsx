// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Markdown in the agent's feed, drawn by a native view. The text splits into top-level blocks as on the desktop, and each
// block renders on its own, so a reply streaming in only lays out its last
// block again. Text stays selectable, a block at a time. A reply's pictures
// show under the block that names them. A sent message's pills show as
// chips its bubble opens, and a reply can offer Quote for what you select.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useMemo } from "react";
import { Linking, StyleSheet, useWindowDimensions, View } from "react-native";
import {
	EnrichedMarkdownText,
	type LinkPressEvent,
	type MarkdownStyle,
	type Md4cFlags,
	type TextContextMenuItem,
} from "react-native-enriched-markdown";

import { splitBlocks } from "@maxhq/shared/agent-view/markdown";
import type { AgentId } from "@maxhq/shared/bindings";

import { PILL_LINK, pillIndex, VISUAL_LINK } from "../features/composer/sent-markdown";
import { visualOfBlock } from "../features/visuals/visual";
import { VisualOpenRow } from "../features/visuals/VisualOpenRow";
import { color, tint } from "../theme";
import { markdownStyle, opensInBrowser, safeBlock } from "./markdown";
import { ReplyPictures } from "./Picture";

const openLink = ({ url }: { url: string }) => {
	if (opensInBrowser(url)) Linking.openURL(url).catch(() => {});
};

// A pill's chip, in its composer pill's shade.
const PILL_CHIPS = {
	[PILL_LINK]: { color: color.primary, backgroundColor: tint(color.primary, 0.16) },
	[VISUAL_LINK]: { color: color.visual, backgroundColor: tint(color.visual, 0.16) },
};

// Math stays text. Your own message keeps its line breaks.
const REPLY: Md4cFlags = { latexMath: false };
const MINE: Md4cFlags = { latexMath: false, hardSoftBreaks: true };

const Block = memo(
	({
		source,
		style,
		mine,
		agent,
		onLinkPress,
		menu,
	}: {
		source: string;
		style: MarkdownStyle;
		mine: boolean;
		agent: AgentId | undefined;
		onLinkPress: (event: LinkPressEvent) => void;
		menu: TextContextMenuItem[] | undefined;
	}) => {
		useI18n();
		const visual = useMemo(() => visualOfBlock(source), [source]);
		const { markdown, pictures, onlyPictures } = useMemo(() => safeBlock(source), [source]);
		if (visual) return <VisualOpenRow visual={visual} />;
		const text =
			agent && onlyPictures ? null : (
				<EnrichedMarkdownText
					markdown={markdown}
					markdownStyle={style}
					flavor="github"
					md4cFlags={mine ? MINE : REPLY}
					selectable
					onLinkPress={onLinkPress}
					contextMenuItems={menu}
					enableLinkPreview={false}
					enableTaskListItemToggle={false}
					spoilerOverlay="solid"
					containerStyle={styles.block}
				/>
			);
		if (!agent || pictures.length === 0) return text;
		return (
			<>
				{text}
				<ReplyPictures agent={agent} pictures={pictures} />
			</>
		);
	}
);
Block.displayName = "MarkdownBlock";

/**
 * Markdown text, rendered block by block: a reply, or your own message. With
 * `agent`, the agent it came from, the pictures it names show too. Pass
 * callbacks that stay the same between renders, so the blocks stay put.
 */
export const MarkdownText = ({
	text,
	mine = false,
	agent,
	quiet,
	onPill,
	onQuote,
}: {
	text: string;
	mine?: boolean;
	agent?: AgentId;
	/** Set for quieter text, such as reasoning: smaller, and in this colour. */
	quiet?: string;
	/** Set for text from sentMarkdown: its pills show as chips, and a tap names the piece. */
	onPill?: (index: number) => void;
	/** Set to offer Quote for the selected text, which it gets. */
	onQuote?: (selected: string) => void;
}) => {
	const { t } = useI18n();
	const { fontScale } = useWindowDimensions();
	const style = useMemo(() => {
		const base = markdownStyle(mine, StyleSheet.hairlineWidth, fontScale, quiet);
		return onPill ? { ...base, linkVariants: { ...base.linkVariants, ...PILL_CHIPS } } : base;
	}, [mine, fontScale, quiet, onPill]);
	const onLinkPress = useMemo(
		() =>
			onPill
				? ({ url }: { url: string }) => {
						const index = pillIndex(url);
						if (index === null) openLink({ url });
						else onPill(index);
					}
				: openLink,
		[onPill]
	);
	const menu = useMemo(
		(): TextContextMenuItem[] | undefined =>
			onQuote && [{ text: t("Quote"), onPress: ({ text: selected }) => onQuote(selected) }],
		[onQuote, t]
	);
	const blocks = useMemo(() => splitBlocks(text), [text]);
	return (
		<View style={styles.markdown}>
			{blocks.map((source, i) => (
				// A block's place is its identity: earlier blocks never change while text streams in.
				<Block
					key={i}
					source={source}
					style={style}
					mine={mine}
					agent={agent}
					onLinkPress={onLinkPress}
					menu={menu}
				/>
			))}
		</View>
	);
};

const styles = StyleSheet.create({
	markdown: { gap: 8, flexShrink: 1, minWidth: 0 },
	block: { flexShrink: 1, minWidth: 0 },
});
