// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A run's steps as the desktop's Workflow tab: a row per node in the
// workflow's order, with its status, its badges and how long it took, and
// a row per group that folds its members away. A tap on a node opens its
// agent, or a sheet with its note and the agents that worked on it when it
// has none of its own. Loops show as a count on the row they went back from.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useCallback, useMemo, useState } from "react";
import { FlatList, Pressable, StyleSheet, Text, View } from "react-native";

import type { AgentId, LoopState, NodeShape, NodeState, RunShape } from "@maxhq/shared/bindings";
import { formatNumber } from "@maxhq/shared/i18n";
import { formatDuration, plural } from "@maxhq/shared/status/format";
import { badgeTone } from "@maxhq/shared/workflow/glyphs";
import {
	groupStatus,
	isSettled,
	loopCount,
	loopTone,
	nodeAgent,
	openAtFirst,
	type TabRow,
	tabRows,
} from "@maxhq/shared/workflow/layout";

import { color, size } from "../../theme";
import { Elapsed } from "../../ui/Elapsed";
import { ChevronDown, ChevronRight, RefreshCw } from "../../ui/glyphs";
import { Badge } from "../test-hosts/parts";
import { NodeGlyph } from "./NodeGlyph";
import { loopsByRow, nodeRowLabel, TONE_TEXT } from "./runs";

const ROW = 40;
// How far a row on the second lane, or inside an open group, sits in.
const INDENT = 16;
const NO_LOOPS: readonly LoopState[] = [];

/** The counts of the loops that went back from a row, which its label says already. */
const LoopCounts = ({ loops }: { loops: readonly LoopState[] }) =>
	loops.length === 0 ? null : (
		<View
			accessibilityElementsHidden
			importantForAccessibility="no-hide-descendants"
			style={styles.loops}
		>
			{loops.map((loop) => {
				const tone = TONE_TEXT[loopTone(loop)];
				return (
					<View key={`${loop.from}-${loop.to}`} style={styles.loop}>
						<RefreshCw size={10} strokeWidth={2.2} color={tone} />
						<Text style={[styles.loopText, { color: tone }]}>{loopCount(loop)}</Text>
					</View>
				);
			})}
		</View>
	);

/** How long the latest attempt took, or a live clock while it runs. */
const AttemptTime = ({ node }: { node: NodeState }) => {
	const attempt = node.attempts.at(-1);
	if (!attempt) return null;
	if (attempt.end === null) {
		return node.status === "busy" ? (
			<Elapsed since={attempt.start} style={{ ...styles.time, color: color.busyText }} />
		) : null;
	}
	return <Text style={styles.time}>{formatDuration(attempt.end - attempt.start)}</Text>;
};

/** One node's row. It renders again only when its own state changes. */
const NodeRow = memo(
	({
		shape,
		node,
		indent,
		loops,
		orchestrator,
		onOpenAgent,
		onSheet,
	}: {
		shape: NodeShape;
		node: NodeState;
		indent: boolean;
		loops: readonly LoopState[];
		orchestrator: AgentId | null;
		onOpenAgent: (agent: AgentId) => void;
		onSheet: (shape: NodeShape, node: NodeState) => void;
	}) => {
		const { t } = useI18n();
		const agent = nodeAgent(node, orchestrator);
		const quiet =
			node.status === "will_skip" || node.status === "skipped" || node.status === "queued";
		const sheet = () => onSheet(shape, node);
		return (
			<Pressable
				accessibilityRole="button"
				accessibilityLabel={nodeRowLabel(shape, node, agent, loops)}
				accessibilityActions={
					agent
						? [
								{
									name: "more",
									label: t("More about {{label}}", { label: shape.label }),
								},
							]
						: []
				}
				onAccessibilityAction={sheet}
				onPress={() => (agent ? onOpenAgent(agent) : sheet())}
				onLongPress={sheet}
				style={({ pressed }) => [
					styles.row,
					indent && styles.indent,
					pressed && styles.pressed,
				]}
			>
				<NodeGlyph status={node.status} provisional={node.provisional} decorative />
				<View style={styles.text}>
					<Text numberOfLines={1} style={[styles.label, quiet && styles.quiet]}>
						{shape.label}
					</Text>
					{node.note ? (
						<Text numberOfLines={1} style={styles.note}>
							{node.note}
						</Text>
					) : null}
				</View>
				{node.badges.slice(0, 2).map((badge) => (
					<Badge key={badge.text} label={badge.text} tone={badgeTone(badge.tone)} />
				))}
				<LoopCounts loops={loops} />
				<AttemptTime node={node} />
				{agent ? (
					<ChevronRight size={14} strokeWidth={2} color={color.subtleForeground} />
				) : null}
			</Pressable>
		);
	}
);
NodeRow.displayName = "NodeRow";

/** A group's row: its name, how many are done, a glyph per member, and a chevron to open it. */
const GroupRow = ({
	row,
	nodes,
	loops,
	onToggle,
}: {
	row: Extract<TabRow, { kind: "group" }>;
	nodes: ReadonlyMap<string, NodeState>;
	loops: readonly LoopState[];
	onToggle: (group: string) => void;
}) => {
	const { t } = useI18n();
	const statuses = row.members.map((member) => nodes.get(member.id)?.status ?? "queued");
	const done = statuses.filter(isSettled).length;
	const Fold = row.open ? ChevronDown : ChevronRight;
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityState={{ expanded: row.open }}
			accessibilityLabel={t("{{label}}, {{done}} of {{value3}} done", {
				label: row.label,
				done: formatNumber(done),
				value3: plural(row.members.length, "node"),
			})}
			onPress={() => onToggle(row.id)}
			style={({ pressed }) => [styles.row, pressed && styles.pressed]}
		>
			<NodeGlyph status={groupStatus(statuses)} decorative />
			<Text numberOfLines={1} style={[styles.label, styles.group]}>
				{row.label}
			</Text>
			<Text style={styles.time}>
				{formatNumber(done)}/{formatNumber(row.members.length)}
			</Text>
			<View style={styles.members}>
				{row.members.map((member, i) => (
					<NodeGlyph
						key={member.id}
						status={statuses[i]!}
						provisional={nodes.get(member.id)?.provisional ?? false}
						size={9}
						decorative
					/>
				))}
			</View>
			<LoopCounts loops={loops} />
			<Fold size={14} strokeWidth={2} color={color.mutedForeground} />
		</Pressable>
	);
};

const keyOf = (row: TabRow) => (row.kind === "group" ? `g:${row.id}` : row.node.id);

/** The steps of one run, scrolling under the run's header. */
export const RunSteps = ({
	shape,
	nodes,
	loops,
	orchestrator,
	onOpenAgent,
	onSheet,
}: {
	shape: RunShape;
	nodes: ReadonlyMap<string, NodeState>;
	loops: readonly LoopState[];
	orchestrator: AgentId | null;
	onOpenAgent: (agent: AgentId) => void;
	onSheet: (shape: NodeShape, node: NodeState) => void;
}) => {
	const { t } = useI18n();
	const [open, setOpen] = useState(() => openAtFirst(shape));
	const rows = useMemo(
		() => tabRows(shape, (id) => nodes.get(id)?.status, open),
		[shape, nodes, open]
	);
	const byRow = useMemo(() => loopsByRow(rows, loops), [rows, loops]);
	const toggle = useCallback(
		(group: string) =>
			setOpen((was) => {
				const next = new Set(was);
				if (!next.delete(group)) next.add(group);
				return next;
			}),
		[]
	);
	return (
		<FlatList
			data={rows}
			extraData={byRow}
			keyExtractor={keyOf}
			accessibilityLabel={t("Steps")}
			contentContainerStyle={styles.content}
			renderItem={({ item, index }) => {
				const rowLoops = byRow.get(index) ?? NO_LOOPS;
				if (item.kind === "group") {
					return <GroupRow row={item} nodes={nodes} loops={rowLoops} onToggle={toggle} />;
				}
				const node = nodes.get(item.node.id);
				if (!node) return null;
				return (
					<NodeRow
						shape={item.node}
						node={node}
						indent={item.grouped || item.lane === 1}
						loops={rowLoops}
						orchestrator={orchestrator}
						onOpenAgent={onOpenAgent}
						onSheet={onSheet}
					/>
				);
			}}
		/>
	);
};

const styles = StyleSheet.create({
	content: { paddingHorizontal: 6, paddingTop: 4, paddingBottom: 32 },
	row: {
		minHeight: ROW,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingHorizontal: 10,
		paddingVertical: 3,
		borderRadius: 6,
	},
	indent: { marginLeft: INDENT },
	pressed: { backgroundColor: color.accent },
	text: { flex: 1, minWidth: 0, gap: 1 },
	label: { color: color.terminal, fontSize: size.small },
	quiet: { color: color.mutedForeground },
	group: { flexShrink: 1 },
	note: { color: color.mutedForeground, fontSize: size.meta },
	time: { color: color.mutedForeground, fontSize: size.pill, fontVariant: ["tabular-nums"] },
	members: {
		flex: 1,
		minWidth: 0,
		flexDirection: "row",
		alignItems: "center",
		gap: 3,
		overflow: "hidden",
	},
	loops: { flexDirection: "row", gap: 6 },
	loop: { flexDirection: "row", alignItems: "center", gap: 2 },
	loopText: { fontSize: size.pill, fontWeight: "600", fontVariant: ["tabular-nums"] },
});
