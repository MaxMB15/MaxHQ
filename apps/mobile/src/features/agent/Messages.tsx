// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What you sent, on the right in a bubble, the files the agent sent you, on
// the left in a frame, and the agent's reply as Markdown across the column,
// with a button under it that copies the reply. A picture in either opens
// whole on a tap. A file you placed in your message's text carries the number
// its pill shows. A pill in what you sent opens its agent, note, pull
// request, visual or picture on a tap, and words you select in a reply go
// in the box below as a quote.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import * as Clipboard from "expo-clipboard";
import { useCallback, useEffect, useMemo } from "react";
import {
	Animated,
	Linking,
	Pressable,
	StyleSheet,
	Text,
	useAnimatedValue,
	View,
} from "react-native";

import { attachmentMeta, formatBytes, sentFilesLine } from "@maxhq/shared/agent-view/format";
import type { AttachmentId, AttachmentRef, TimelineItem } from "@maxhq/shared/bindings";
import { copyLabel, useCopyWith } from "@maxhq/shared/lib/copy";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { makeQuote } from "@maxhq/shared/pills/quote";
import { sentMessage } from "@maxhq/shared/pills/sent";
import { formatClock } from "@maxhq/shared/status/format";

import type { Screens } from "../../navigation";
import { useView } from "../../services";
import { color, radius, size } from "../../theme";
import { Check, CircleAlert, Copy, File, FileText, Folder, ImageIcon } from "../../ui/glyphs";
import { opensInBrowser, withCaret } from "../../ui/markdown";
import { MarkdownText } from "../../ui/MarkdownText";
import { useReduceMotion } from "../../ui/motion";
import { PictureThumbs, showPicture } from "../../ui/Picture";
import { useBoxShown } from "../composer/open-boxes";
import { quoteOfSelection } from "../composer/quote";
import { sentMarkdown } from "../composer/sent-markdown";
import { openVisual, visualOfItem } from "../visuals/visual";
import { useFeed } from "./feed-context";

type ItemOf<K extends TimelineItem["kind"]> = Extract<TimelineItem, { kind: K }>;

const FILE_ICON = { pdf: FileText, file: File, folder: Folder, image: ImageIcon } as const;

// What a file is, with its size, and first the number its pill shows, such as "PDF 1, 412 KB".
const metaOf = (file: AttachmentRef, number: number | undefined): string =>
	number === undefined
		? attachmentMeta(file)
		: t("{{type}} {{number}}, {{size}}", {
				type: file.kind === "pdf" ? "PDF" : t("Image"),
				number: formatNumber(number),
				size: formatBytes(file.bytes),
			});

const FileChip = ({ file, number }: { file: AttachmentRef; number: number | undefined }) => {
	const Icon = FILE_ICON[file.kind];
	return (
		<View style={styles.chip}>
			<View style={styles.chipIcon}>
				<Icon size={14} strokeWidth={1.8} color={color.secondaryForeground} />
			</View>
			<View style={styles.chipText}>
				<Text numberOfLines={1} style={styles.chipName}>
					{file.name}
				</Text>
				<Text numberOfLines={1} style={styles.chipMeta}>
					{metaOf(file, number)}
				</Text>
			</View>
		</View>
	);
};

// The pictures in a row above the other files.
const Attachments = ({
	files,
	numbers,
}: {
	files: readonly AttachmentRef[];
	/** The number each placed image and PDF shows on its pill. */
	numbers?: ReadonlyMap<AttachmentId, number>;
}) => {
	const { agent } = useFeed();
	const others = files.filter((file) => file.kind !== "image");
	return (
		<>
			<PictureThumbs
				agent={agent}
				files={files.filter((file) => file.kind === "image")}
				numbers={numbers}
			/>
			{others.length > 0 ? (
				<View style={styles.chips}>
					{others.map((file) => (
						<FileChip key={file.id} file={file} number={numbers?.get(file.id)} />
					))}
				</View>
			) : null}
		</>
	);
};

/** A message you sent. */
export const UserMessage = ({ item }: { item: ItemOf<"user_message"> }) => {
	const { t } = useI18n();
	const { agent } = useFeed();
	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const agents = useView((state) => state.view.tables.agent);

	const at = item.started_at ?? item.completed_at;
	const sent = useMemo(() => sentMessage(item), [item]);
	const text = useMemo(
		() => sentMarkdown(sent.pieces, sent.factsOf, sent.numbers, (id) => agents.get(id)?.title),
		[sent, agents]
	);
	// A tap on a pill's chip opens what it names, where the phone can.
	const onPill = useCallback(
		(index: number) => {
			const piece = sent.pieces[index];
			if (!piece || piece.type === "text") return;
			if (piece.type === "git") {
				if (opensInBrowser(piece.ref.url)) Linking.openURL(piece.ref.url).catch(() => {});
				return;
			}
			const { ref } = piece;
			if (ref.type === "agent") navigation.push("Agent", { agent: ref.agent });
			else if (ref.type === "note")
				navigation.push("Note", { project: ref.project, note: ref.note });
			else if (ref.type === "visual") {
				const file = sent.files.get(ref.attachment);
				if (file) openVisual(visualOfItem(agent, { title: ref.title, attachment: file }));
			} else if (ref.type === "attachment") {
				const file = sent.files.get(ref.id);
				if (file?.kind === "image") showPicture(agent, file);
			}
		},
		[agent, navigation, sent]
	);
	return (
		<View style={styles.mine}>
			<View style={[styles.bubble, styles.bubbleMine]}>
				<Attachments files={sent.shown} numbers={sent.numbers} />
				{text ? <MarkdownText text={text} mine onPill={onPill} /> : null}
			</View>
			{at !== null ? (
				<Text style={[styles.time, styles.timeMine]}>
					{item.delivery === "steer"
						? t("{{time}}, sent while it worked", { time: formatClock(at) })
						: formatClock(at)}
				</Text>
			) : null}
		</View>
	);
};

/** Files the agent sent you, with the message it sent with them. */
export const AgentAttachments = ({ item }: { item: ItemOf<"agent_attachments"> }) => {
	const at = item.completed_at ?? item.started_at;
	const sent = sentFilesLine(item.attachments.length);
	return (
		<View style={styles.theirs}>
			<View style={[styles.bubble, styles.bubbleTheirs]}>
				<Attachments files={item.attachments} />
				{item.text ? <MarkdownText text={item.text} /> : null}
			</View>
			<Text style={[styles.time, styles.timeTheirs]}>
				{at !== null ? `${sent}, ${formatClock(at)}` : sent}
			</Text>
		</View>
	);
};

// "Writing", fading in and out, until the first words arrive.
const Writing = () => {
	const { t } = useI18n();

	const opacity = useAnimatedValue(1);
	const still = useReduceMotion();
	useEffect(() => {
		if (still) {
			opacity.setValue(1);
			return;
		}
		const pulse = Animated.loop(
			Animated.sequence([
				Animated.timing(opacity, { toValue: 0.5, duration: 1000, useNativeDriver: true }),
				Animated.timing(opacity, { toValue: 1, duration: 1000, useNativeDriver: true }),
			])
		);
		pulse.start();
		return () => pulse.stop();
	}, [opacity, still]);
	return <Animated.Text style={[styles.writing, { opacity }]}>{t("Writing")}</Animated.Text>;
};

const COPY_ICON = { idle: Copy, copied: Check, failed: CircleAlert } as const;

const CopyReply = ({ text }: { text: string }) => {
	const { t } = useI18n();

	const [state, copy] = useCopyWith(Clipboard.setStringAsync);
	const Icon = COPY_ICON[state];
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={state === "idle" ? t("Copy the reply as Markdown") : undefined}
			hitSlop={8}
			onPress={() => copy(text)}
			style={styles.copy}
		>
			<Icon size={12} strokeWidth={1.9} color={color.mutedForeground} />
			<Text style={styles.copyText}>{copyLabel(state, t("Copy"))}</Text>
		</Pressable>
	);
};

/**
 * The agent's reply, and once it has finished, a button that copies its
 * Markdown. While the agent's box shows, words you select in it can go
 * there as a quote.
 */
export const AssistantMessage = ({ item }: { item: ItemOf<"assistant_message"> }) => {
	const { agent, readOnly } = useFeed();
	const title = useView((state) => state.view.tables.agent.get(agent)?.title ?? "");
	const boxShown = useBoxShown(agent);
	const onQuote = useCallback(
		(selected: string) => {
			const anchor = quoteOfSelection(item.text, selected);
			if (anchor) quoteInbox.send(agent, makeQuote({ id: agent, title }, item, anchor), true);
		},
		[agent, item, title]
	);
	if (item.text === "" && item.streaming) return <Writing />;
	return (
		<View style={styles.reply}>
			<MarkdownText
				text={item.streaming ? withCaret(item.text) : item.text}
				agent={agent}
				onQuote={!readOnly && boxShown && !item.streaming ? onQuote : undefined}
			/>
			{!item.streaming && item.text.trim() !== "" ? <CopyReply text={item.text} /> : null}
		</View>
	);
};

const styles = StyleSheet.create({
	mine: { alignItems: "flex-end", gap: 4 },
	theirs: { alignItems: "flex-start", gap: 4 },
	bubble: {
		maxWidth: "78%",
		minWidth: 0,
		gap: 9,
		paddingHorizontal: 12,
		paddingVertical: 10,
		borderRadius: 12,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.track,
	},
	bubbleMine: { backgroundColor: color.secondary },
	bubbleTheirs: { backgroundColor: color.card },
	time: { color: color.mutedForeground, fontSize: size.pill, fontVariant: ["tabular-nums"] },
	timeMine: { paddingRight: 4 },
	timeTheirs: { paddingLeft: 4 },
	chips: { flexDirection: "row", flexWrap: "wrap", gap: 8 },
	chip: {
		height: 40,
		maxWidth: 260,
		minWidth: 0,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingLeft: 8,
		paddingRight: 12,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.card,
	},
	chipIcon: {
		width: 26,
		height: 26,
		alignItems: "center",
		justifyContent: "center",
		borderRadius: 5,
		backgroundColor: color.border,
	},
	chipText: { flexShrink: 1, minWidth: 0 },
	chipName: { color: color.foreground, fontSize: size.meta },
	chipMeta: { color: color.mutedForeground, fontSize: size.pill },
	writing: { color: color.mutedForeground, fontSize: size.small },
	reply: { gap: 4 },
	copy: {
		height: 22,
		alignSelf: "flex-start",
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
	},
	copyText: { color: color.mutedForeground, fontSize: size.tiny },
});
