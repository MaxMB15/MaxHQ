// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One run over time, as the desktop's timeline: a row per node and a bar
// per attempt, colored by how it ended, with the time it waits on you
// hatched amber. A bar with an agent opens it. What the run doesn't
// record, the timeline says below.

import { useI18n } from "@maxhq/shared/i18n";

import { memo, useMemo, useState } from "react";
import {
	FlatList,
	type LayoutChangeEvent,
	Pressable,
	StyleSheet,
	Text,
	useWindowDimensions,
	View,
} from "react-native";
import Svg, { Defs, Path, Pattern, Rect } from "react-native-svg";

import type { AgentId, NodeState, RunShape, Timestamp } from "@maxhq/shared/bindings";
import {
	barLabel,
	type TimelineBar,
	type TimelineRow,
	timeline,
} from "@maxhq/shared/workflow/canvas";
import { glyphOf } from "@maxhq/shared/workflow/glyphs";

import { color, size, tint, TOUCH } from "../../theme";
import { barGeometry, GLYPH_TINT, type TimeAxis, timeAxis } from "./runs";

const ROW = 40;
const BAR = 14;
const AXIS = 22;
const PAD = 12;
const TICK_LABEL = 60;

/** The part of a bar spent waiting on you, hatched amber. */
const Hatch = ({ width }: { width: number }) => (
	<Svg width={width} height={BAR}>
		<Defs>
			<Pattern
				id="wait"
				width={6}
				height={6}
				patternUnits="userSpaceOnUse"
				patternTransform="rotate(45)"
			>
				<Rect width={6} height={6} fill={tint(color.busy, 0.25)} />
				<Path d="M 0 0 V 6" strokeWidth={2.5} stroke={color.busy} />
			</Pattern>
		</Defs>
		<Rect width={width} height={BAR} fill="url(#wait)" />
	</Svg>
);

/** One attempt's bar, which opens its agent when it has one. */
const Bar = ({
	bar,
	index,
	axis,
	onOpenAgent,
}: {
	bar: TimelineBar;
	index: number;
	axis: TimeAxis;
	onOpenAgent: (agent: AgentId) => void;
}) => {
	const { left, width, solid } = barGeometry(bar, axis.x);
	const ink = GLYPH_TINT[glyphOf(bar.status).color];
	const label = barLabel(bar, index);
	const box = [styles.bar, { left, width }];
	const body = (
		<>
			{solid > 0 ? (
				<View
					style={{
						width: solid,
						height: BAR,
						backgroundColor: ink,
						opacity: bar.running ? 1 : 0.75,
					}}
				/>
			) : null}
			{bar.waitFrom === null || width - solid <= 0 ? null : <Hatch width={width - solid} />}
			{bar.running && bar.waitFrom === null ? <View style={styles.end} /> : null}
		</>
	);
	const agent = bar.agent;
	if (!agent) {
		return (
			<View accessible accessibilityRole="image" accessibilityLabel={label} style={box}>
				{body}
			</View>
		);
	}
	const y = (TOUCH - BAR) / 2;
	const x = Math.max(0, (TOUCH - width) / 2);
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={label}
			hitSlop={{ top: y, bottom: y, left: x, right: x }}
			onPress={() => onOpenAgent(agent)}
			style={({ pressed }) => [...box, pressed && styles.pressed]}
		>
			{body}
		</Pressable>
	);
};

/** One node's row: its name, then its attempts on the axis. */
const Row = memo(
	({
		row,
		axis,
		labels,
		onOpenAgent,
	}: {
		row: TimelineRow;
		axis: TimeAxis;
		labels: number;
		onOpenAgent: (agent: AgentId) => void;
	}) => (
		<View style={styles.row}>
			<Text numberOfLines={1} style={[styles.name, { width: labels }]}>
				{row.node.label}
			</Text>
			<View style={styles.chart}>
				<View
					accessibilityElementsHidden
					importantForAccessibility="no-hide-descendants"
					style={StyleSheet.absoluteFill}
				>
					{axis.ticks.map((tick) => (
						<View key={tick} style={[styles.tick, { left: axis.x(tick) }]} />
					))}
				</View>
				{row.bars.map((bar, i) => (
					<Bar
						key={bar.start}
						bar={bar}
						index={i}
						axis={axis}
						onOpenAgent={onOpenAgent}
					/>
				))}
			</View>
		</View>
	)
);
Row.displayName = "TimelineRow";

/** The clock times along the top, over the chart. */
const Axis = ({ axis, labels, chart }: { axis: TimeAxis; labels: number; chart: number }) => (
	<View
		accessibilityElementsHidden
		importantForAccessibility="no-hide-descendants"
		style={[styles.axis, { marginLeft: labels }]}
	>
		{axis.ticks.map((tick) => (
			<Text
				key={tick}
				numberOfLines={1}
				style={[
					styles.tickLabel,
					{
						left: Math.min(
							Math.max(0, axis.x(tick) - TICK_LABEL / 2),
							chart - TICK_LABEL
						),
					},
				]}
			>
				{axis.label(tick)}
			</Text>
		))}
	</View>
);

/** The run's attempts on a time axis. */
export const RunTimeline = ({
	shape,
	nodes,
	now,
	onOpenAgent,
}: {
	shape: RunShape;
	nodes: ReadonlyMap<string, NodeState>;
	now: Timestamp;
	onOpenAgent: (agent: AgentId) => void;
}) => {
	const { t } = useI18n();
	const screen = useWindowDimensions();
	const [width, setWidth] = useState(screen.width);
	const line = useMemo(() => timeline(shape, nodes, now), [shape, nodes, now]);
	const inner = Math.max(0, width - 2 * PAD);
	const labels = Math.min(120, Math.round(inner * 0.35));
	const chart = Math.max(80, inner - labels);
	const axis = useMemo(
		() => timeAxis(line.start, line.end, chart),
		[line.start, line.end, chart]
	);
	const empty = line.rows.every((row) => row.bars.length === 0);
	return (
		<FlatList
			onLayout={(e: LayoutChangeEvent) => setWidth(e.nativeEvent.layout.width)}
			data={line.rows}
			keyExtractor={(row) => row.node.id}
			accessibilityLabel={t("Attempts over time")}
			contentContainerStyle={styles.content}
			ListHeaderComponent={<Axis axis={axis} labels={labels} chart={chart} />}
			renderItem={({ item }) => (
				<Row row={item} axis={axis} labels={labels} onOpenAgent={onOpenAgent} />
			)}
			ListFooterComponent={
				<View style={styles.notes}>
					{empty ? <Text style={styles.empty}>{t("Nothing has run yet.")}</Text> : null}
					{line.missing.map((text) => (
						<Text key={text} style={styles.missing}>
							{text}.
						</Text>
					))}
				</View>
			}
		/>
	);
};

const styles = StyleSheet.create({
	content: { paddingHorizontal: PAD, paddingTop: 6, paddingBottom: 32 },
	axis: { height: AXIS },
	tickLabel: {
		position: "absolute",
		top: 2,
		width: TICK_LABEL,
		textAlign: "center",
		color: color.mutedForeground,
		fontSize: 10.5,
		fontVariant: ["tabular-nums"],
	},
	row: { height: ROW, flexDirection: "row", alignItems: "center" },
	name: { color: color.terminal, fontSize: size.meta, paddingRight: 8, textAlign: "right" },
	chart: { flex: 1, height: ROW, justifyContent: "center" },
	tick: { position: "absolute", top: 0, bottom: 0, width: 1, backgroundColor: color.track },
	bar: {
		position: "absolute",
		top: (ROW - BAR) / 2,
		height: BAR,
		flexDirection: "row",
		borderRadius: 3,
		overflow: "hidden",
	},
	end: {
		position: "absolute",
		right: 0,
		top: 0,
		width: 2,
		height: BAR,
		backgroundColor: color.foreground,
	},
	pressed: { opacity: 0.7 },
	notes: { gap: 6, paddingTop: 10 },
	empty: { color: color.mutedForeground, fontSize: size.tiny },
	missing: { color: color.busyText, fontSize: size.tiny },
});
