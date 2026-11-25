// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The quieter items: errors, compaction, a stopped turn, a fresh session, the
// line that ends a turn, notices from the CLI, a move to another vendor, and
// anything MaxHQ doesn't know how to show yet.

import { useI18n } from "@maxhq/shared/i18n";

import type { ReactNode } from "react";
import { StyleSheet, Text, View } from "react-native";

import { handoffEnd, handoffTitle, isOpen, newSessionLine } from "@maxhq/shared/agent-view/format";
import type { TimelineItem, Vendor } from "@maxhq/shared/bindings";
import { formatDuration, formatTokens } from "@maxhq/shared/status/format";
import { modelName } from "@maxhq/shared/view/select";

import { useView } from "../../services";
import { color, font, radius, size, tint } from "../../theme";
import { Spinner } from "../../ui/Dot";
import {
	ArrowRightLeft,
	CircleAlert,
	CircleStop,
	Info,
	RotateCcw,
	Shrink,
	TriangleAlert,
} from "../../ui/glyphs";
import { SignInAgain } from "./SignInAgain";

type ItemOf<K extends TimelineItem["kind"]> = Extract<TimelineItem, { kind: K }>;

const SMALL = { size: 12, strokeWidth: 1.9, color: color.mutedForeground } as const;

/** Something that went wrong. */
export const ErrorRow = ({ item }: { item: ItemOf<"error"> }) => {
	const { t } = useI18n();
	return (
		<View style={styles.error}>
			<View style={styles.mark}>
				<CircleAlert size={14} strokeWidth={1.9} color={color.destructive} />
			</View>
			<View style={styles.errorWords}>
				<Text selectable style={styles.errorText}>
					{item.message}
				</Text>
				{item.sign_in ? <SignInAgain account={item.sign_in} /> : null}
			</View>
			{item.retrying ? (
				<View style={styles.retrying}>
					<Spinner size={10} tone={color.destructive} />
					<Text style={styles.retryingText}>{t("Retrying")}</Text>
				</View>
			) : null}
		</View>
	);
};

/** A line across the column with a few words in the middle. */
export const Divider = ({ icon, children }: { icon: ReactNode; children: ReactNode }) => {
	return (
		<View style={styles.divider}>
			<View style={styles.rule} />
			<View style={styles.dividerMiddle}>
				{icon}
				<Text style={styles.dividerText}>{children}</Text>
			</View>
			<View style={styles.rule} />
		</View>
	);
};

/** The CLI shrinking the conversation to fit. */
export const CompactionRow = ({ item }: { item: ItemOf<"compaction"> }) => {
	const { t } = useI18n();
	return (
		<Divider icon={isOpen(item.status) ? <Spinner size={10} /> : <Shrink {...SMALL} />}>
			{isOpen(item.status)
				? t("Compacting the conversation")
				: item.before_tokens !== null && item.after_tokens !== null
					? t("Compacted the conversation, {{before}} to {{after}} tokens", {
							before: formatTokens(item.before_tokens),
							after: formatTokens(item.after_tokens),
						})
					: t("Compacted the conversation")}
		</Divider>
	);
};

/** A turn you stopped. */
export const InterruptedRow = () => {
	const { t } = useI18n();
	return <Divider icon={<CircleStop {...SMALL} />}>{t("You stopped this turn")}</Divider>;
};

/** Where a task's run started the agent on a fresh session. */
export const NewSessionRow = () => {
	useI18n();
	return <Divider icon={<RotateCcw {...SMALL} />}>{newSessionLine()}</Divider>;
};

/** The line that ends a turn. */
export const TurnSummaryRow = ({ item }: { item: ItemOf<"turn_summary"> }) => {
	const { t } = useI18n();

	const parts: string[] = [];
	if (item.duration_ms !== null)
		parts.push(t("Worked for {{duration}}", { duration: formatDuration(item.duration_ms) }));
	if (item.context_tokens !== null)
		parts.push(
			t("{{number}} tokens of context", {
				count: item.context_tokens,
				number: formatTokens(item.context_tokens),
			})
		);
	return <Divider icon={null}>{parts.length > 0 ? parts.join(", ") : t("Turn ended")}</Divider>;
};

/** A message from the CLI or maxd. */
export const NoticeRow = ({ item }: { item: ItemOf<"notice"> }) => {
	const warning = item.level === "warning";
	return (
		<View style={styles.notice}>
			<View style={styles.mark}>
				{warning ? (
					<TriangleAlert size={14} strokeWidth={1.9} color={color.busy} />
				) : (
					<Info size={14} strokeWidth={1.9} color={color.mutedForeground} />
				)}
			</View>
			<Text selectable style={[styles.noticeText, warning ? styles.warning : null]}>
				{item.message}
			</Text>
		</View>
	);
};

// One end of a move, with the model by its label.
const HandoffEnd = ({ vendor, model }: { vendor: Vendor; model: string | null }) => {
	const name = useView((s) => (model ? modelName(s.view, vendor, model) : null));
	return <Text style={styles.handoffEnd}>{handoffEnd(vendor, name)}</Text>;
};

/**
 * Where the agent moved to another vendor. It waits until the next message
 * carries the conversation over.
 */
export const HandoffRow = ({ item }: { item: ItemOf<"handoff"> }) => {
	return (
		<Divider icon={<ArrowRightLeft {...SMALL} />}>
			{handoffTitle(item.status)}{" "}
			<HandoffEnd vendor={item.from_vendor} model={item.from_model} />
			{" → "}
			<HandoffEnd vendor={item.to_vendor} model={item.to_model} />
		</Divider>
	);
};

/** Something the driver can't show yet. */
export const UnknownRow = ({ item }: { item: ItemOf<"unknown"> }) => {
	const { t } = useI18n();
	return (
		<View style={styles.unknown}>
			<Info size={14} strokeWidth={1.9} color={color.mutedForeground} />
			<Text style={styles.noticeText}>{t("MaxHQ can't show this item yet")}</Text>
			<Text numberOfLines={1} selectable style={styles.nativeKind}>
				{item.native_kind}
			</Text>
		</View>
	);
};

const styles = StyleSheet.create({
	error: {
		flexDirection: "row",
		alignItems: "flex-start",
		gap: 10,
		paddingHorizontal: 12,
		paddingVertical: 8,
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: tint(color.destructive, 0.3),
		backgroundColor: tint(color.destructive, 0.06),
	},
	mark: { marginTop: 3 },
	errorWords: { flex: 1, minWidth: 0, gap: 8 },
	errorText: { color: color.foreground, fontSize: 12.5, lineHeight: 19 },
	retrying: { flexDirection: "row", alignItems: "center", gap: 6, paddingTop: 2 },
	retryingText: { color: color.errorText, fontSize: size.tiny },
	divider: { flexDirection: "row", alignItems: "center", gap: 12, paddingVertical: 2 },
	rule: { flex: 1, minWidth: 12, height: 1, backgroundColor: color.border },
	dividerMiddle: { flexShrink: 1, flexDirection: "row", alignItems: "center", gap: 6 },
	dividerText: {
		flexShrink: 1,
		color: color.mutedForeground,
		fontSize: size.tiny,
		fontVariant: ["tabular-nums"],
		textAlign: "center",
	},
	notice: { flexDirection: "row", alignItems: "flex-start", gap: 8, paddingHorizontal: 6 },
	noticeText: {
		flexShrink: 1,
		minWidth: 0,
		color: color.mutedForeground,
		fontSize: 12.5,
		lineHeight: 19,
	},
	warning: { color: color.busyText },
	handoffEnd: { color: color.secondaryForeground, fontWeight: "500" },
	unknown: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 6 },
	nativeKind: {
		flexShrink: 1,
		minWidth: 0,
		color: color.subtleForeground,
		fontFamily: font.mono,
		fontSize: size.pill,
	},
});
