// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A file change's patch, as on the desktop: added lines on a green tint and
// removed lines on a red one, the code in colour once the highlighter has
// it and plain until then, and hunk headers dimmed, in monospace that
// scrolls sideways rather than wraps. Each hunk is highlighted on its own,
// both sides of it, so the colours stay right across added and removed
// lines. It shows the first lines with a button for more, a page at a time,
// so a long patch keeps the feed quick, and names each file when there are
// several. The hunks already shown stay as they are when more shows.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { Fragment, memo, useMemo, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View, type ViewStyle } from "react-native";

import { extensionOf } from "@maxhq/shared/agent-view/viewer-files";
import type { FileEdit } from "@maxhq/shared/bindings";
import {
	editPath,
	PATCH_LINES,
	PATCH_PAGE,
	patchFiles,
	type PatchLine,
	patchRuns,
	patchSides,
	shownCounts,
	shownPatch,
} from "@maxhq/shared/timeline/diff";

import { color, font, radius, size, tint } from "../../theme";
import { type Language, languageOf, tokenStyle, useHighlight } from "../../ui/highlight";

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

// Lines per text block at most. A view per line would make thousands for a
// long patch, and one block for all of it a layer too tall to draw.
const BLOCK = 100;

const SIGN: Record<PatchLine["type"], string> = { hunk: "", add: "+", remove: "-", context: " " };

/** One hunk, or part of a long one, in runs of lines of one kind. */
const Chunk = memo(({ lines, lang }: { lines: readonly PatchLine[]; lang: Language | null }) => {
	const sides = useMemo(() => patchSides(lines), [lines]);
	const before = useHighlight(sides.old ?? "", lang, sides.old !== null);
	const after = useHighlight(sides.new ?? "", lang, sides.new !== null);
	const runs = useMemo(() => patchRuns(lines, BLOCK), [lines]);
	return (
		<>
			{runs.map(({ type, start, end }) => (
				// A run's place in its chunk is its identity, as the chunk never changes.
				<View key={start} style={RUN[type]}>
					<Text selectable style={[styles.code, LINE[type]]}>
						{lines.slice(start, end).map((line, i) => {
							const spot = sides.spots[start + i];
							const tokens =
								spot && (spot.side === "old" ? before : after)?.[spot.line];
							return (
								// Lines have no identity but their place in the run.
								<Fragment key={i}>
									{type === "hunk" ? null : (
										<Text style={SIGN_COLOR[type]}>{SIGN[type]}</Text>
									)}
									{tokens
										? tokens.map((token, j) => (
												<Text key={j} style={tokenStyle(token)}>
													{token.content}
												</Text>
											))
										: line.text}
									{start + i < end - 1 ? "\n" : ""}
								</Fragment>
							);
						})}
					</Text>
				</View>
			))}
		</>
	);
});
Chunk.displayName = "PatchChunk";

/** One file's patch, chunk by chunk. */
const FileLines = memo(
	({ chunks, path }: { chunks: readonly (readonly PatchLine[])[]; path: string }) => {
		const lang = languageOf(extensionOf(path));
		return (
			<ScrollView
				horizontal
				showsHorizontalScrollIndicator={false}
				contentContainerStyle={styles.scroll}
			>
				<View style={styles.lines}>
					{chunks.map((chunk, i) => (
						// Chunks have no identity but their place in the patch.
						<Chunk key={i} lines={chunk} lang={lang} />
					))}
				</View>
			</ScrollView>
		);
	}
);
FileLines.displayName = "PatchFileLines";

/** The patches of `changes`, the first lines until "Show all" or "Show more". */
export const PatchView = ({
	changes,
	style,
}: {
	changes: readonly FileEdit[];
	style?: ViewStyle;
}) => {
	const { t } = useI18n();

	const files = useMemo(() => patchFiles(changes), [changes]);
	// The most lines shown, null for all of them.
	const [limit, setLimit] = useState<number | null>(PATCH_LINES);
	const patch = useMemo(() => shownPatch(files, limit), [files, limit]);
	const paged = limit !== null && patch.hidden > PATCH_PAGE;
	const several = changes.length > 1;
	return (
		<View style={[styles.patch, style]}>
			{patch.files.map(({ change, chunks }, i) => (
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
					{chunks === null ? (
						<Text style={styles.note}>
							{t("The CLI didn't send a diff for this file.")}
						</Text>
					) : chunks.length > 0 ? (
						<FileLines chunks={chunks} path={change.path} />
					) : null}
				</View>
			))}
			{patch.more ? (
				<Pressable
					accessibilityRole="button"
					onPress={() => setLimit(paged ? limit + PATCH_PAGE : null)}
					hitSlop={8}
					style={styles.more}
				>
					<Text style={styles.moreText}>
						{paged
							? t("Show {{value2}} more", { value2: formatNumber(PATCH_PAGE) })
							: t("Show all")}
					</Text>
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
	// At least as wide as the screen, so a tinted line reaches across it.
	scroll: { flexGrow: 1 },
	lines: { flexGrow: 1 },
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

// Each kind of line's tint, the desktop's 8% of its colour.
const RUN = StyleSheet.create({
	hunk: { paddingHorizontal: 10 },
	add: { paddingHorizontal: 10, backgroundColor: tint(color.ready, 0.08) },
	remove: { paddingHorizontal: 10, backgroundColor: tint(color.destructive, 0.08) },
	context: { paddingHorizontal: 10 },
});

// Each kind of line's colour while it's plain.
const LINE = StyleSheet.create({
	hunk: { color: color.subtleForeground },
	add: { color: color.readyText },
	remove: { color: color.errorText },
	context: { color: color.secondaryForeground },
});

const SIGN_COLOR = StyleSheet.create({
	hunk: {},
	add: { color: color.ready },
	remove: { color: color.destructive },
	context: {},
});
