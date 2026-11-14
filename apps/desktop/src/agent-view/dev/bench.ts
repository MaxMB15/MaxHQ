// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent view benchmark. With eight views open and the lead agent
// streaming, it times every animation frame through a few phases: sitting
// still, scrolling up through the 10,000 item history, scrolling every view
// at once, and jumping around the history. It logs p50, p95 and dropped
// frames for each phase, and posts them to `?report=` when that's set.

import { LONG_HISTORY } from "@maxhq/shared/data/mock-timeline";

/** Frame times for one phase, in milliseconds. */
export type BenchResult = {
	readonly phase: string;
	readonly frames: number;
	readonly p50: number;
	readonly p95: number;
	readonly p99: number;
	readonly max: number;
	/** Frames that took more than one and a half intervals. */
	readonly long: number;
	/** Intervals that passed without a frame. */
	readonly dropped: number;
};

const VIEWS = 8;
const WARM_UP_MS = 1500;

// Browsers report this when work a resize observer causes waits for the
// next frame. The virtualizer measures rows that way, so it's expected.
const HARMLESS = /ResizeObserver loop/;

const frame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));

const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

const scrollerIn = (agent: string) =>
	document.querySelector<HTMLElement>(`[data-agent="${agent}"] [role="region"]`);

const scrollers = () => [...document.querySelectorAll<HTMLElement>('[data-agent] [role="region"]')];

// Waits until every view shows rows.
const ready = async () => {
	const until = performance.now() + 20_000;
	while (performance.now() < until) {
		const all = scrollers();
		if (all.length >= VIEWS && all.every((el) => el.querySelector("[data-index]"))) return;
		await delay(100);
	}
	throw new Error("The views didn't open in time.");
};

// Times each frame for `ms`, calling `step` before every one.
const record = async (ms: number, step?: (n: number) => void) => {
	const times: number[] = [];
	let last = await frame();
	const end = last + ms;
	for (let n = 0; last < end; n++) {
		step?.(n);
		const now = await frame();
		times.push(now - last);
		last = now;
	}
	return times;
};

const percentile = (sorted: readonly number[], p: number) =>
	sorted[Math.min(sorted.length - 1, Math.floor((sorted.length - 1) * p))] ?? 0;

// The display's frame interval, from the shortest frames seen.
const intervalOf = (times: readonly number[]) => {
	const low = percentile(
		[...times].sort((a, b) => a - b),
		0.1
	);
	const rates = [1000 / 120, 1000 / 90, 1000 / 60, 1000 / 30];
	return rates.reduce((best, rate) =>
		Math.abs(rate - low) < Math.abs(best - low) ? rate : best
	);
};

const summarize = (phase: string, times: readonly number[], interval: number): BenchResult => {
	const sorted = [...times].sort((a, b) => a - b);
	return {
		phase,
		frames: times.length,
		p50: percentile(sorted, 0.5),
		p95: percentile(sorted, 0.95),
		p99: percentile(sorted, 0.99),
		max: sorted[sorted.length - 1] ?? 0,
		long: times.filter((t) => t > interval * 1.5).length,
		dropped: times.reduce((sum, t) => sum + Math.max(0, Math.round(t / interval) - 1), 0),
	};
};

// Scrolls the way a wheel does, so the view stops following.
const wheel = (el: HTMLElement, dy: number) => {
	el.dispatchEvent(new WheelEvent("wheel", { deltaY: dy, bubbles: true }));
	el.scrollTop += dy;
};

const send = async (report: string | null, body: unknown) => {
	if (!report) return;
	await fetch(report, { method: "POST", body: JSON.stringify(body) }).catch(console.error);
};

/** Sends errors on the page to `report` too, since a webview's console is hard to reach. */
export const reportErrors = (report: string | null) => {
	window.addEventListener("error", (event) => {
		if (!HARMLESS.test(event.message)) void send(report, { error: event.message });
	});
	window.addEventListener(
		"unhandledrejection",
		(event) => void send(report, { error: String(event.reason) })
	);
};

type Phase = readonly [name: string, ms: number, step?: (n: number) => void];

const phasesFor = (history: HTMLElement, all: readonly HTMLElement[]): Phase[] => [
	["Streaming, still", 5000],
	["Scrolling up the history", 10_000, () => wheel(history, -60)],
	[
		"Scrolling all eight",
		6000,
		(n) => {
			const dy = Math.floor(n / 30) % 2 === 0 ? -40 : 40;
			for (const el of all) wheel(el, dy);
		},
	],
	[
		"Jumping in the history",
		4000,
		(n) => {
			if (n % 10 !== 0) return;
			history.dispatchEvent(new WheelEvent("wheel", { deltaY: -1, bubbles: true }));
			history.scrollTop = Math.random() * (history.scrollHeight - history.clientHeight);
		},
	],
];

const bench = async (report: string | null, runs: number): Promise<BenchResult[]> => {
	await ready();
	const warm = await record(WARM_UP_MS);
	const interval = intervalOf(warm);

	const history = scrollerIn(LONG_HISTORY);
	if (!history) throw new Error("The long history isn't open.");
	const all = scrollers();

	const results: BenchResult[] = [];
	for (let run = 1; run <= runs; run++) {
		if (run > 1) {
			for (const el of all) el.scrollTop = el.scrollHeight;
			await delay(500);
		}
		for (const [name, ms, step] of phasesFor(history, all)) {
			const phase = runs > 1 ? `${name}, run ${run}` : name;
			results.push(summarize(phase, await record(ms, step), interval));
		}
	}

	const out = {
		interval,
		views: all.length,
		historyRows: history.querySelectorAll("[data-index]").length,
		// Code blocks on screen, and how many of them Shiki colored.
		codeBlocks: document.querySelectorAll("pre > code").length,
		highlighted: document.querySelectorAll("pre > code:has(span[style*='color'])").length,
		userAgent: navigator.userAgent,
		results,
	};
	console.table(results);
	console.info("Bench frame interval", interval.toFixed(2), "ms");
	(window as { benchResult?: typeof out }).benchResult = out;
	await send(report, out);
	return results;
};

let running: Promise<BenchResult[]> | null = null;

/**
 * Runs the benchmark `runs` times on the open views and gives one result
 * per phase and run. A page runs it once, however often this is called.
 */
export const runBench = (report: string | null, runs = 1): Promise<BenchResult[]> => {
	running ??= bench(report, runs).catch(async (error: unknown) => {
		console.error(error);
		await send(report, { error: String(error) });
		return [];
	});
	return running;
};
