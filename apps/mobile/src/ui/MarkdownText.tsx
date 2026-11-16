// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Markdown in the agent's feed, drawn by a native view. The text splits into top-level blocks as on the desktop, and each
// block renders on its own, so a reply streaming in only lays out its last
// block again. Text stays selectable, a block at a time. A reply's pictures
// show under the block that names them. Fenced code shows in a card with
// its colours and Copy, a `mermaid` fence as a diagram, and display math
// drawn, which the native view can't do. A link to a file on the agent's
// device opens it in the viewer. A sent message's pills show as chips its
// bubble opens, and a reply can offer Quote for what you select.

import { useI18n } from "@maxhq/shared/i18n";

import { NavigationContext } from "@react-navigation/native";
import { memo, useCallback, useContext, useMemo } from "react";
import { Linking, StyleSheet, useWindowDimensions, View } from "react-native";
import {
	EnrichedMarkdownText,
	type MarkdownStyle,
	type Md4cFlags,
	type TaskListItemPressEvent,
	type TextContextMenuItem,
} from "react-native-enriched-markdown";

import { type CodeFence, codeFenceOf } from "@maxhq/shared/agent-view/markdown";
import { localFileLink } from "@maxhq/shared/agent-view/viewer-files";
import type { AgentId } from "@maxhq/shared/bindings";
import { taskCount } from "@maxhq/shared/notes/tasks";

import { PILL_LINK, pillIndex, VISUAL_LINK } from "../features/composer/sent-markdown";
import { Diagram } from "../features/diagrams/Diagram";
import { MathBlock } from "../features/diagrams/MathBlock";
import { fileRoute } from "../features/viewer/file";
import { visualOfBlock } from "../features/visuals/visual";
import { VisualOpenRow } from "../features/visuals/VisualOpenRow";
import { color, tint } from "../theme";
import { CodeBlock } from "./CodeBlock";
import { markdownPieces, markdownStyle, opensInBrowser, safeBlock } from "./markdown";
import { ReplyPictures } from "./Picture";

// A pill's chip, in its composer pill's shade.
const PILL_CHIPS = {
	[PILL_LINK]: { color: color.primary, backgroundColor: tint(color.primary, 0.16) },
	[VISUAL_LINK]: { color: color.visual, backgroundColor: tint(color.visual, 0.16) },
};

// Math stays text. Your own message keeps its line breaks.
const REPLY: Md4cFlags = { latexMath: false };
const MINE: Md4cFlags = { latexMath: false, hardSoftBreaks: true };

/** Hears a task box ticked: its number in the whole text, and whether it's done now. */
export type TaskTicked = (index: number, done: boolean) => void;

type OpenLink = (event: { url: string }) => void;

// A fenced block as the desktop draws it: a diagram, a formula once it's
// whole, or code.
const Fence = ({ fence }: { fence: CodeFence }) => {
	const lang = fence.lang?.toLowerCase();
	if (lang === "mermaid") return <Diagram source={fence.code} live={!fence.closed} />;
	if (lang === "math" && fence.closed) return <MathBlock tex={fence.code} />;
	return <CodeBlock code={fence.code} lang={fence.lang} live={!fence.closed} />;
};

const Block = memo(
	({
		source,
		style,
		mine,
		agent,
		firstTask,
		onTask,
		onLink,
		menu,
	}: {
		source: string;
		style: MarkdownStyle;
		mine: boolean;
		agent: AgentId | undefined;
		/** The number of the block's first task box in the whole text. */
		firstTask: number;
		onTask: TaskTicked | undefined;
		onLink: OpenLink;
		menu: TextContextMenuItem[] | undefined;
	}) => {
		useI18n();
		const visual = useMemo(() => visualOfBlock(source), [source]);
		const fence = useMemo(() => codeFenceOf(source), [source]);
		const { markdown, pictures, onlyPictures } = useMemo(() => safeBlock(source), [source]);
		// The view ticks its own box at once and says how it stands now.
		const tick = useMemo(
			() =>
				onTask &&
				(({ index, checked }: TaskListItemPressEvent) =>
					onTask(firstTask + index, checked)),
			[onTask, firstTask]
		);
		if (visual) return <VisualOpenRow visual={visual} />;
		if (fence) return <Fence fence={fence} />;
		const text =
			agent && onlyPictures ? null : (
				<EnrichedMarkdownText
					markdown={markdown}
					markdownStyle={style}
					flavor="github"
					md4cFlags={mine ? MINE : REPLY}
					selectable
					onLinkPress={onLink}
					contextMenuItems={menu}
					enableLinkPreview={false}
					enableTaskListItemToggle={tick !== undefined}
					onTaskListItemPress={tick}
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
 * `agent`, the agent it came from, the pictures it names show too, and a
 * link to one of its files opens that file. Pass callbacks that stay the
 * same between renders, so the blocks stay put.
 */
export const MarkdownText = ({
	text,
	mine = false,
	agent,
	folder,
	quiet,
	onTask,
	onPill,
	onQuote,
}: {
	text: string;
	mine?: boolean;
	agent?: AgentId;
	/** The folder a relative link starts from, as a Markdown file's own. */
	folder?: string;
	/** Set for quieter text, such as reasoning: smaller, and in this colour. */
	quiet?: string;
	/** Set to let a tap tick the task boxes. */
	onTask?: TaskTicked;
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
	const menu = useMemo(
		(): TextContextMenuItem[] | undefined =>
			onQuote && [{ text: t("Quote"), onPress: ({ text: selected }) => onQuote(selected) }],
		[onQuote, t]
	);
	// Outside a screen there's nowhere to open a file.
	const navigation = useContext(NavigationContext);
	const onLink = useCallback<OpenLink>(
		({ url }) => {
			if (opensInBrowser(url)) {
				Linking.openURL(url).catch(() => {});
				return;
			}
			const link = agent && navigation ? localFileLink(url, folder) : null;
			const route = agent && link ? fileRoute(agent, link.path, link.line) : null;
			if (route) navigation?.navigate("File", route);
		},
		[agent, folder, navigation]
	);
	const onLinkPress = useMemo<OpenLink>(
		() =>
			onPill
				? ({ url }) => {
						const index = pillIndex(url);
						if (index === null) onLink({ url });
						else onPill(index);
					}
				: onLink,
		[onPill, onLink]
	);
	const pieces = useMemo(() => markdownPieces(text), [text]);
	// Each block numbers its own boxes from 0, so it learns how many come before it.
	const firstTasks = useMemo(() => {
		if (!onTask) return null;
		let count = 0;
		return pieces.map((piece) => {
			const first = count;
			if (piece.kind === "block") count += taskCount(piece.source);
			return first;
		});
	}, [pieces, onTask]);
	return (
		<View style={styles.markdown}>
			{pieces.map((piece, i) =>
				// A piece's place is its identity: earlier ones never change while text streams in.
				piece.kind === "math" ? (
					<MathBlock key={`math ${i}`} tex={piece.tex} live={piece.live} />
				) : (
					<Block
						key={i}
						source={piece.source}
						style={style}
						mine={mine}
						agent={agent}
						firstTask={firstTasks?.[i] ?? 0}
						onTask={onTask}
						onLink={onLinkPress}
						menu={menu}
					/>
				)
			)}
		</View>
	);
};

const styles = StyleSheet.create({
	markdown: { gap: 8, flexShrink: 1, minWidth: 0 },
	block: { flexShrink: 1, minWidth: 0 },
});
