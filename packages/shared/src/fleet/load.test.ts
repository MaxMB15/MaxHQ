// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ProcessUse } from "../bindings";
import { agent, entities, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import { cpuBars, loadStats, processRows } from "./load";
import type { Diagnostics } from "./wire";

const GB = 1024 ** 3;

const use = (over: Partial<ProcessUse> & { pid: number }): ProcessUse => ({
	agent: null,
	workspace: null,
	command: "node server.js",
	cpu_now: 0,
	cpu_avg: 0,
	cpu_max: 0,
	rss_now: GB,
	rss_max: GB,
	...over,
});

const diagnostics = (top: ProcessUse[]): Diagnostics => ({
	buckets: [],
	top,
	containers: [],
	error: null,
});

describe("loadStats", () => {
	it("waits with dots until the device answers", () => {
		expect(loadStats(null, null).map((s) => s.value)).toEqual(["...", "...", "...", "..."]);
	});

	it("shows the CPU, cores, free memory and agents, warning near full", () => {
		const load = {
			sampled_at: NOW,
			cpu_util: 0.93,
			cpu_count: 8,
			mem_available: 14 * GB,
			mem_total: 32 * GB,
		};
		const stats = loadStats(load, diagnostics([use({ pid: 1, agent: "a" }), use({ pid: 2 })]));
		expect(stats.map((s) => [s.label, s.value, s.warning])).toEqual([
			["CPU", "93%", true],
			["Cores", "8", false],
			["Memory free", "14.0 GB", false],
			["Agents", "1", false],
		]);
	});
});

describe("cpuBars", () => {
	it("scales to the busiest bar and keeps an idle one visible", () => {
		const bars = cpuBars([
			{ started_at: 1, ended_at: 2, avg_cpu: 50, max_cpu: 200, max_rss: 0, max_processes: 1 },
			{ started_at: 2, ended_at: 3, avg_cpu: 0, max_cpu: 0, max_rss: 0, max_processes: 0 },
		]);
		expect(bars.map((b) => [b.average, b.peak])).toEqual([
			[25, 100],
			[2, 2],
		]);
		expect(bars[0]!.label).toBe("Average 50%, peak 200%");
	});
});

describe("processRows", () => {
	it("puts the busiest first and names each by its agent or its command", () => {
		const view = applySnapshot({
			seq: 1,
			entities: entities({ agents: [agent({ id: "a", title: "Fix the build" })] }),
		});
		const rows = processRows(
			diagnostics([use({ pid: 1, cpu_now: 5 }), use({ pid: 2, agent: "a", cpu_now: 80 })]),
			view
		);
		expect(rows.map((r) => [r.name, r.cpu])).toEqual([
			["Fix the build", "80%"],
			["node", "5%"],
		]);
	});
});
