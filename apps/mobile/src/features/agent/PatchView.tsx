// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A file change's patch, plain: added lines green, removed lines red and
// hunk headers dimmed, in monospace that scrolls sideways rather than
// wraps. It shows the first lines with a button for the rest, so a long
// patch keeps the feed quick, and names each file when there are several.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { useMemo, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View, type ViewStyle } from "react-native";

import type { FileEdit } from "@maxhq/shared/bindings";
import {
	editPath,
	PATCH_LINES,
	type PatchLine,
	shownCounts,
	shownPatch,
} from "@maxhq/shared/timeline/diff";

import { color, font, radius, size } from "../../theme";

/** Lines added and removed, such as "+3 −1", as the desktop shows them. */
export const Counts = ({ additions, deletions }: { additions: number; deletions: number }) => {
	useI18n();
	const shown = shownCounts(additions, deletions);
	return (
		<Text style={styles.counts}>
			{shown.additions !== null ? (
				<Text style={styles.added}>+{formatNumber(shown.additions)}</Text>
			) : null}
			{shown.additions !== null && shown.deletions !== null ? " " : null}
			{shown.deletions !== null ? (
				<Text style={styles.removed}>−{formatNumber(shown.deletions)}</Text>
			) : null}
		</Text>
	);
};

// Lines per text block. A view per line would make thousands for a long
// patch, and one block for all of it a layer too tall to draw.
const BLOCK = 100;

const SIGN: Record<PatchLine["type"], string> = { hunk: "", add: "+", remove: "-", context: " " };

const Lines = ({ lines }: { lines: readonly PatchLine[] }) => {
	const blocks: (readonly PatchLine[])[] = [];
	for (let i = 0; i < lines.length; i += BLOCK) blocks.push(lines.slice(i, i + BLOCK));
	return blocks.map((block, b) => (
		// Blocks and lines have no identity but their place in the patch.
		<Text key={b} selectable style={styles.code}>
			{block.map((line, i) => (
				<Text key={i} style={LINE[line.type]}>
					{SIGN[line.type]}
					{line.text}
					{i < block.length - 1 ? "\n" : ""}
				</Text>
			))}
		</Text>
	));
};

/** The patches of `changes`, the first lines until "Show all". */
export const PatchView = ({
	changes,
	style,
}: {
	changes: readonly FileEdit[];
	style?: ViewStyle;
}) => {
	const { t } = useI18n();

	const [all, setAll] = useState(false);
	const patch = useMemo(() => shownPatch(changes, all ? null : PATCH_LINES), [changes, all]);
	const several = changes.length > 1;
	return (
		<View style={[styles.patch, style]}>
			{patch.files.map(({ change, lines }, i) => (
				// Edits have no ID of their own; their place in the item never changes.
				<View key={i} style={styles.file}>
					{several ? (
						<View style={styles.fileHead}>
							<Text style={styles.path} numberOfLines={1}>
								{editPath(change)}
							</Text>
							<Counts additions={change.additions} deletions={change.deletions} />
						</View>
					) : null}
					{lines === null ? (
						<Text style={styles.note}>
							{t("The CLI didn't send a diff for this file.")}
						</Text>
					) : lines.length > 0 ? (
						<ScrollView
							horizontal
							showsHorizontalScrollIndicator={false}
							contentContainerStyle={styles.scroll}
						>
							<View>
								<Lines lines={lines} />
							</View>
						</ScrollView>
					) : null}
				</View>
			))}
			{patch.more ? (
				<Pressable
					accessibilityRole="button"
					onPress={() => setAll(true)}
					hitSlop={8}
					style={styles.more}
				>
					<Text style={styles.moreText}>{t("Show all")}</Text>
				</Pressable>
			) : null}
		</View>
	);
};

const styles = StyleSheet.create({
	counts: { fontFamily: font.mono, fontSize: size.tiny },
	added: { color: color.ready },
	removed: { color: color.destructive },
	patch: {
		paddingVertical: 8,
		gap: 8,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
	},
	file: { gap: 4 },
	fileHead: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 10 },
	path: {
		flexShrink: 1,
		color: color.terminal,
		fontFamily: font.mono,
		fontSize: size.tiny,
	},
	scroll: { paddingHorizontal: 10 },
	code: { fontFamily: font.mono, fontSize: size.tiny, lineHeight: 17 },
	note: { paddingHorizontal: 10, color: color.mutedForeground, fontSize: size.tiny },
	more: {
		alignSelf: "flex-start",
		minHeight: 28,
		justifyContent: "center",
		paddingHorizontal: 10,
	},
	moreText: { color: color.primary, fontSize: size.meta, fontWeight: "600" },
});

const LINE = StyleSheet.create({
	hunk: { color: color.subtleForeground },
	add: { color: color.readyText },
	remove: { color: color.errorText },
	context: { color: color.secondaryForeground },
});
