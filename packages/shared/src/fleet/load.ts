// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the stat blocks,
// the CPU history's average and peak bars and the process table, as the
// numbers and words each client draws. MaxHQ reads one device's `maxd`
// instead of the local server, and names each process tree by the agent
// that runs it.

import { t } from "../i18n";

import type { HostLoad, Timestamp, UseBucket } from "../bindings";
import type { ViewState } from "../view/replica";
import { formatMemory } from "./memory";
import type { Diagnostics } from "./wire";

/** How often the meters read the device again. */
export const LOAD_REFRESH_MS = 5000;
/** The history covers the last hour in two-minute bars. */
export const LOAD_WINDOW_SECS = 3600;
export const LOAD_BUCKET_SECS = 120;

/** A share of one CPU, or of all of them, as "42%". */
export const percent = (value: number): string => `${Math.round(value)}%`;

/** One block over the history. */
export type LoadStat = {
	readonly label: string;
	readonly value: string;
	/** Set when the CPUs are nearly full. */
	readonly warning: boolean;
};

/** CPU, cores, free memory and agents, with "..." for what hasn't come yet. */
export const loadStats = (
	load: HostLoad | null | undefined,
	diagnostics: Diagnostics | null
): LoadStat[] => {
	const cpu = load?.cpu_util ?? null;
	const agents = diagnostics?.top.filter((p) => p.agent !== null).length ?? 0;
	return [
		{
			label: "CPU",
			value: cpu === null ? "..." : percent(cpu * 100),
			warning: cpu !== null && cpu >= 0.9,
		},
		{ label: t("Cores"), value: load ? String(load.cpu_count) : "...", warning: false },
		{
			label: t("Memory free"),
			value: load ? formatMemory(load.mem_available) : "...",
			warning: false,
		},
		{ label: t("Agents"), value: diagnostics ? String(agents) : "...", warning: false },
	];
};

/** One bar of the CPU history, its heights in percent of the chart's. */
export type CpuBar = {
	readonly at: Timestamp;
	readonly average: number;
	readonly peak: number;
	/** The bar in words, such as "Average 12%, peak 40%". */
	readonly label: string;
};

/** The history's bars, scaled to the busiest one, never thinner than 2%. */
export const cpuBars = (buckets: readonly UseBucket[]): CpuBar[] => {
	const top = Math.max(1, ...buckets.map((b) => b.max_cpu));
	return buckets.map((bucket) => ({
		at: bucket.started_at,
		peak: Math.max(2, (bucket.max_cpu / top) * 100),
		average: Math.max(2, (bucket.avg_cpu / top) * 100),
		label: t("Average {{value1}}, peak {{value2}}", {
			value1: percent(bucket.avg_cpu),
			value2: percent(bucket.max_cpu),
		}),
	}));
};

/** One row of the busiest processes. */
export type ProcessRow = {
	readonly pid: number;
	/** The agent's title, or the command's first word. */
	readonly name: string;
	readonly command: string;
	readonly cpu: string;
	readonly peak: string;
	readonly memory: string;
};

/** The busiest processes, the busiest now first, each named by its agent. */
export const processRows = (diagnostics: Diagnostics, view: ViewState): ProcessRow[] =>
	[...diagnostics.top]
		.sort((a, b) => b.cpu_now - a.cpu_now)
		.map((p) => ({
			pid: p.pid,
			name:
				(p.agent ? view.tables.agent.get(p.agent)?.title : undefined) ??
				p.command.split(" ")[0] ??
				p.command,
			command: p.command,
			cpu: percent(p.cpu_now),
			peak: percent(p.cpu_max),
			memory: formatMemory(p.rss_now),
		}));
