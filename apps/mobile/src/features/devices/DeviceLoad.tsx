// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the stat blocks,
// the CPU history's average and peak bars and the process table, drawn for
// the phone from the shared package's `fleet/load`, as the desktop's
// device page does. MaxHQ reads one device's `maxd` instead of the local
// server, and names each process tree by the agent that runs it. The
// meters read again only while the page is in front and the app is open.

import { useI18n } from "@maxhq/shared/i18n";

import { useIsFocused } from "@react-navigation/native";
import { memo, useEffect, useState } from "react";
import { StyleSheet, Text, View } from "react-native";
import Svg, { Rect } from "react-native-svg";

import type { DeviceLoad as Sample } from "@maxhq/shared/data/source";
import {
	cpuBars,
	LOAD_BUCKET_SECS,
	LOAD_REFRESH_MS,
	LOAD_WINDOW_SECS,
	loadStats,
	type ProcessRow,
	processRows,
} from "@maxhq/shared/fleet/load";
import type { Diagnostics, UseBucket } from "@maxhq/shared/fleet/wire";

import { useAppActive, useServices } from "../../services";
import { color, font, radius, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { Buttons, CardBody, Muted, type Note, NoteLine, type SectionProps } from "./section";

const CHART_HEIGHT = 96;

const History = ({ buckets }: { buckets: readonly UseBucket[] }) => {
	const { t } = useI18n();

	const bars = cpuBars(buckets);
	return (
		<CardBody>
			<View
				accessible
				accessibilityRole="image"
				accessibilityLabel={t("CPU over the last hour")}
				style={styles.chart}
			>
				<Svg
					width="100%"
					height={CHART_HEIGHT - 16}
					viewBox={`0 0 ${bars.length} 100`}
					preserveAspectRatio="none"
				>
					{bars.map((bar, i) => (
						<Rect
							key={`peak-${bar.at}`}
							x={i + 0.1}
							y={100 - bar.peak}
							width={0.8}
							height={bar.peak}
							fill={tint(color.foreground, 0.15)}
						/>
					))}
					{bars.map((bar, i) => (
						<Rect
							key={`average-${bar.at}`}
							x={i + 0.1}
							y={100 - bar.average}
							width={0.8}
							height={bar.average}
							fill={tint(color.foreground, 0.6)}
						/>
					))}
				</Svg>
			</View>
		</CardBody>
	);
};

const ProcessLine = memo(function ProcessLine({ row }: { row: ProcessRow }) {
	return (
		<View style={[styles.process, styles.line]} accessible>
			<Text numberOfLines={1} style={styles.processName}>
				{row.name}
			</Text>
			<Text style={styles.number}>{row.cpu}</Text>
			<Text style={[styles.number, styles.muted]}>{row.peak}</Text>
			<Text style={[styles.number, styles.memory]}>{row.memory}</Text>
		</View>
	);
});

const Processes = ({ rows }: { rows: readonly ProcessRow[] }) => {
	const { t } = useI18n();
	return (
		<View accessibilityLabel={t("Busiest processes")}>
			<View style={[styles.process, styles.line]}>
				<Text style={[styles.processName, styles.head]}>{t("Agent or process")}</Text>
				<Text style={[styles.number, styles.head]}>CPU</Text>
				<Text style={[styles.number, styles.head]}>{t("Peak")}</Text>
				<Text style={[styles.number, styles.memory, styles.head]}>{t("Memory")}</Text>
			</View>
			{rows.map((row) => (
				<ProcessLine key={row.pid} row={row} />
			))}
		</View>
	);
};

/** A device's CPU and memory now, its last hour and its busiest agents. */
export const DeviceLoad = ({ device, name, live, view }: SectionProps) => {
	const { t } = useI18n();

	const { source } = useServices();
	const focused = useIsFocused();
	const active = useAppActive();
	const reading = live && focused && active;
	const [sample, setSample] = useState<Sample | null>(null);
	const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);
	const [note, setNote] = useState<Note | null>(null);
	const [reads, setReads] = useState(0);

	useEffect(() => {
		if (!reading) return;
		let gone = false;
		const read = () =>
			void source.readLoad([device]).then(
				(loads) => {
					if (!gone) setSample(loads.find((l) => l.device === device) ?? null);
				},
				(error: unknown) => console.warn("Couldn't read the device's load", error)
			);
		read();
		const timer = setInterval(read, LOAD_REFRESH_MS);
		return () => {
			gone = true;
			clearInterval(timer);
		};
	}, [source, device, reading]);

	useEffect(() => {
		if (!live) return;
		let gone = false;
		void source.readDiagnostics(device, LOAD_WINDOW_SECS, LOAD_BUCKET_SECS).then(
			(result) => {
				if (gone) return;
				if (result.status === "ok") {
					setDiagnostics(result.data);
					setNote(result.data.error ? { text: result.data.error, error: true } : null);
				} else {
					setNote({
						text: t("Couldn't read {{name}}'s history. Try again.", { name: name }),
						error: true,
					});
				}
			},
			(error: unknown) => {
				console.warn("Couldn't read the device's history", error);
				if (!gone) setNote({ text: t("Couldn't reach it. Try again."), error: true });
			}
		);
		return () => {
			gone = true;
		};
	}, [source, device, name, live, reads, t]);

	const load = live ? sample?.load : null;
	return (
		<Card
			title={t("Load")}
			note={live ? t("Every 5 seconds, history for the last hour") : undefined}
			label={t("Load on {{name}}", { name: name })}
		>
			{!live ? (
				<CardBody>
					<Muted>{t("Connect to {{name}} to see its load.", { name: name })}</Muted>
				</CardBody>
			) : (
				<>
					<View style={[styles.stats, styles.line]}>
						{loadStats(load, diagnostics).map((stat) => (
							<View
								key={stat.label}
								style={styles.stat}
								accessible
								accessibilityLabel={`${stat.label}: ${stat.value}`}
							>
								<Text numberOfLines={1} style={styles.statLabel}>
									{stat.label.toUpperCase()}
								</Text>
								<Text
									numberOfLines={1}
									style={[styles.statValue, stat.warning && styles.warning]}
								>
									{stat.value}
								</Text>
							</View>
						))}
					</View>
					{diagnostics && diagnostics.buckets.length > 0 ? (
						<History buckets={diagnostics.buckets} />
					) : null}
					{diagnostics && diagnostics.top.length > 0 ? (
						<Processes rows={processRows(diagnostics, view)} />
					) : null}
					<CardBody>
						<NoteLine note={note} />
						<Buttons>
							<Button
								label={t("Read history again")}
								small
								onPress={() => setReads((n) => n + 1)}
							/>
						</Buttons>
					</CardBody>
				</>
			)}
		</Card>
	);
};

const styles = StyleSheet.create({
	line: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	stats: { flexDirection: "row", flexWrap: "wrap" },
	stat: { width: "50%", paddingHorizontal: 14, paddingVertical: 10, gap: 3 },
	statLabel: {
		color: color.mutedForeground,
		fontSize: 10.5,
		fontWeight: "500",
		letterSpacing: 1,
	},
	statValue: {
		color: color.foreground,
		fontFamily: font.mono,
		fontSize: 17,
		fontWeight: "600",
		fontVariant: ["tabular-nums"],
	},
	warning: { color: color.connectingText },
	chart: {
		height: CHART_HEIGHT,
		padding: 8,
		borderRadius: radius.sm,
		backgroundColor: color.subpanel,
		overflow: "hidden",
	},
	process: {
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingHorizontal: 14,
		paddingVertical: 7,
	},
	processName: { flex: 1, color: color.foreground, fontSize: size.meta },
	head: { color: color.mutedForeground, fontFamily: undefined, fontSize: size.tiny },
	number: {
		width: 44,
		textAlign: "right",
		color: color.foreground,
		fontFamily: font.mono,
		fontSize: size.meta,
		fontVariant: ["tabular-nums"],
	},
	muted: { color: color.mutedForeground },
	memory: { width: 64 },
});
