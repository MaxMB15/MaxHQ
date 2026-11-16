// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the stat blocks,
// the CPU history's average and peak bars, the process table, rewritten as
// arrow functions in MaxHQ's cards, with their numbers and words in the
// shared package's `fleet/load`, which the phone draws too. MaxHQ reads one
// device's `maxd` instead of the local server, and names each process tree by
// the agent that runs it.

import { useI18n } from "@maxhq/shared/i18n";

import { useEffect, useState } from "react";

import { useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import { SECONDARY, Section, StatusNote } from "@/settings/controls";
import type { DeviceId } from "@maxhq/shared/bindings";
import type { DeviceLoad as Sample } from "@maxhq/shared/data/source";
import {
	cpuBars,
	LOAD_BUCKET_SECS,
	LOAD_REFRESH_MS,
	LOAD_WINDOW_SECS,
	loadStats,
	processRows,
} from "@maxhq/shared/fleet/load";
import type { Diagnostics, UseBucket } from "@maxhq/shared/fleet/wire";
import type { ViewState } from "@maxhq/shared/view/replica";
import type { Note } from "@/settings/result";

const StatBlock = ({
	label,
	value,
	tone = "default",
}: {
	label: string;
	value: string;
	tone?: "default" | "warning";
}) => {
	useI18n();
	return (
		<div role="group" aria-label={label} className="min-w-0 px-4 py-3">
			<div className="truncate text-[10.5px] font-medium tracking-widest text-muted-foreground uppercase">
				{label}
			</div>
			<div
				className={cn(
					"mt-1 truncate font-mono text-lg font-semibold text-foreground tabular-nums",
					tone === "warning" && "text-status-connecting-text"
				)}
			>
				{value}
			</div>
		</div>
	);
};

const History = ({ buckets }: { buckets: readonly UseBucket[] }) => {
	const { t } = useI18n();

	return (
		<div
			role="img"
			aria-label={t("CPU over the last hour")}
			className="border-t border-sidebar-border px-4 py-3"
		>
			<div className="flex h-24 items-end gap-0.5 overflow-hidden rounded-sm bg-subpanel p-2">
				{cpuBars(buckets).map((bar) => (
					<div
						key={bar.at}
						title={bar.label}
						className="relative h-full min-w-0.5 flex-1"
					>
						<div
							className="absolute inset-x-0 bottom-0 rounded-t-sm bg-foreground/15"
							style={{ height: `${bar.peak}%` }}
						/>
						<div
							className="absolute inset-x-0 bottom-0 rounded-t-sm bg-foreground/60"
							style={{ height: `${bar.average}%` }}
						/>
					</div>
				))}
			</div>
		</div>
	);
};

const Processes = ({ diagnostics, view }: { diagnostics: Diagnostics; view: ViewState }) => {
	const { t } = useI18n();

	return (
		<table
			aria-label={t("Busiest processes")}
			className="w-full border-t border-sidebar-border text-xs"
		>
			<thead>
				<tr className="text-left text-[11px] text-muted-foreground">
					<th className="px-4 py-2 font-medium">{t("Agent or process")}</th>
					<th className="px-2 py-2 text-right font-medium">CPU</th>
					<th className="px-2 py-2 text-right font-medium">{t("Peak")}</th>
					<th className="px-4 py-2 text-right font-medium">{t("Memory")}</th>
				</tr>
			</thead>
			<tbody>
				{processRows(diagnostics, view).map((p) => (
					<tr key={p.pid} className="border-t border-row-divider">
						<td
							className="max-w-0 truncate px-4 py-1.5 text-foreground"
							title={p.command}
						>
							{p.name}
						</td>
						<td className="px-2 py-1.5 text-right font-mono tabular-nums">{p.cpu}</td>
						<td className="px-2 py-1.5 text-right font-mono text-muted-foreground tabular-nums">
							{p.peak}
						</td>
						<td className="px-4 py-1.5 text-right font-mono tabular-nums">
							{p.memory}
						</td>
					</tr>
				))}
			</tbody>
		</table>
	);
};

/** A device's CPU and memory now, its last hour and its busiest agents. */
export const DeviceLoad = ({
	device,
	name,
	live,
	view,
}: {
	device: DeviceId;
	name: string;
	live: boolean;
	view: ViewState;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [sample, setSample] = useState<Sample | null>(null);
	const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);
	const [note, setNote] = useState<Note | null>(null);
	const [reads, setReads] = useState(0);

	useEffect(() => {
		if (!live) return;
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
	}, [source, device, live]);

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
		<Section
			title={t("Load")}
			label={t("Load on {{name}}", { name: name })}
			hint={live ? t("Every 5 seconds, history for the last hour") : undefined}
		>
			{!live ? (
				<p className="px-4 py-3 text-xs text-muted-foreground">
					{t("Connect to {{name}} to see its load.", { name: name })}
				</p>
			) : (
				<>
					<div className="grid grid-cols-4 divide-x divide-sidebar-border">
						{loadStats(load, diagnostics).map((stat) => (
							<StatBlock
								key={stat.label}
								label={stat.label}
								value={stat.value}
								tone={stat.warning ? "warning" : "default"}
							/>
						))}
					</div>
					{diagnostics && diagnostics.buckets.length > 0 ? (
						<History buckets={diagnostics.buckets} />
					) : null}
					{diagnostics && diagnostics.top.length > 0 ? (
						<Processes diagnostics={diagnostics} view={view} />
					) : null}
					<div className="flex items-center justify-end gap-2.5 border-t border-sidebar-border px-4 py-2.5">
						<StatusNote note={note} />
						<button
							type="button"
							className={SECONDARY}
							onClick={() => setReads((n) => n + 1)}
						>
							{t("Read history again")}
						</button>
					</div>
				</>
			)}
		</Section>
	);
};
