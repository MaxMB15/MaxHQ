// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Markdown in the agent's feed, drawn by a native view. The text splits into top-level blocks as on the desktop, and each
// block renders on its own, so a reply streaming in only lays out its last
// block again. Text stays selectable, a block at a time. A reply's pictures
// show under the block that names them.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useMemo } from "react";
import { Linking, StyleSheet, useWindowDimensions, View } from "react-native";
import {
	EnrichedMarkdownText,
	type MarkdownStyle,
	type Md4cFlags,
} from "react-native-enriched-markdown";

import { splitBlocks } from "@maxhq/shared/agent-view/markdown";
import type { AgentId } from "@maxhq/shared/bindings";

import { visualOfBlock } from "../features/visuals/visual";
import { VisualOpenRow } from "../features/visuals/VisualOpenRow";
import { markdownStyle, opensInBrowser, safeBlock } from "./markdown";
import { ReplyPictures } from "./Picture";

const openLink = ({ url }: { url: string }) => {
	if (opensInBrowser(url)) Linking.openURL(url).catch(() => {});
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
	}: {
		source: string;
		style: MarkdownStyle;
		mine: boolean;
		agent: AgentId | undefined;
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
					onLinkPress={openLink}
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
 * `agent`, the agent it came from, the pictures it names show too.
 */
export const MarkdownText = ({
	text,
	mine = false,
	agent,
	quiet,
}: {
	text: string;
	mine?: boolean;
	agent?: AgentId;
	/** Set for quieter text, such as reasoning: smaller, and in this colour. */
	quiet?: string;
}) => {
	const { fontScale } = useWindowDimensions();
	const style = useMemo(
		() => markdownStyle(mine, StyleSheet.hairlineWidth, fontScale, quiet),
		[mine, fontScale, quiet]
	);
	const blocks = useMemo(() => splitBlocks(text), [text]);
	return (
		<View style={styles.markdown}>
			{blocks.map((source, i) => (
				// A block's place is its identity: earlier blocks never change while text streams in.
				<Block key={i} source={source} style={style} mine={mine} agent={agent} />
			))}
		</View>
	);
};

const styles = StyleSheet.create({
	markdown: { gap: 8, flexShrink: 1, minWidth: 0 },
	block: { flexShrink: 1, minWidth: 0 },
});
