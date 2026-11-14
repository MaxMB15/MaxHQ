// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Agent views on their own, on the sample data, for a look without the rest
// of the app. Only the dev server serves this page, at
// /src/agent-view/dev/preview.html. Add `?agents=a,b` to pick the views, or
// `?bench` to open eight and run the benchmark, with `&runs=2` to run it
// twice and `&report=<url>` to post the results there.

import { formatNumber } from "@maxhq/shared/i18n";
import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ServicesProvider, useView } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import {
	createMockTimelines,
	LONG_HISTORY,
	MOCK_CLIENT,
	STREAMING,
} from "@maxhq/shared/data/mock-timeline";
import type { DataSource } from "@maxhq/shared/data/source";
import type { Entity } from "@maxhq/shared/bindings";
import "@/styles/globals.css";

import { AgentView } from "../agent-view";
import { type BenchResult, reportErrors, runBench } from "./bench";

// Agents MaxHQ holds here, so their cards can be answered.
const HELD = new Set([
	"agent-usage-popover",
	"agent-hdr",
	"agent-release",
	"agent-sync-drift",
	STREAMING,
]);
// A session from a terminal that has ended, for the banner that offers to continue it.
const ENDED = "agent-sidebar-sort";

const PREVIEW = ["agent-usage-popover", "agent-hdr"];
const BENCH = [
	LONG_HISTORY,
	STREAMING,
	"agent-usage-popover",
	"agent-hdr",
	"agent-release",
	"agent-sync-drift",
	"agent-bindings",
	ENDED,
];

const adjust = (entity: Entity, now: number): Entity => {
	if (entity.entity !== "agent") return entity;
	if (HELD.has(entity.id)) return { ...entity, control: "held" };
	if (entity.id === ENDED) {
		return {
			...entity,
			link: { state: "down", reason: "exited", since: now - 2 * 60_000 },
			vendor_session_id: entity.vendor_session_id ?? "6b1f0c2e-58a4-4d7e-9a35-0e8f2b7d41c9",
		};
	}
	return entity;
};

const previewSource = (): DataSource => {
	const base = mockSource();
	const timelines = createMockTimelines({ now: Date.now, stream: true });
	return {
		...base,
		snapshot: async () => {
			const snapshot = await base.snapshot();
			return { ...snapshot, entities: snapshot.entities.map((e) => adjust(e, Date.now())) };
		},
		subscribe: (onDelta) =>
			base.subscribe((delta) =>
				onDelta({
					...delta,
					changes: delta.changes.map((change) =>
						change.op === "upsert"
							? { ...change, value: adjust(change.value, Date.now()) }
							: change
					),
				})
			),
		openTimeline: async (agent, onUpdate) => timelines.open(agent, onUpdate),
		loadOlder: async (agent, before) => timelines.older(agent, before),
		answer: async (attention, answer) => {
			timelines.decide(attention, answer, MOCK_CLIENT);
			return { status: "ok", data: { status: "done" } };
		},
	};
};

const Pane = ({ id }: { id: string }) => {
	const agent = useView((s) => s.view.tables.agent.get(id));
	return (
		<div data-agent={id} className="min-h-0 min-w-0 bg-background">
			{agent ? (
				<AgentView agent={agent} />
			) : (
				<p className="p-4 text-[12px] text-muted-foreground">No agent {id}</p>
			)}
		</div>
	);
};

const Results = ({ results }: { results: readonly BenchResult[] }) => (
	<table className="absolute top-2 right-2 z-50 rounded-md border border-input bg-popover text-[11.5px] tabular-nums shadow-lg [&_td]:px-2 [&_td]:py-0.5 [&_th]:px-2 [&_th]:py-1 [&_th]:text-left">
		<thead>
			<tr>
				<th>Phase</th>
				<th>Frames</th>
				<th>p50 ms</th>
				<th>p95 ms</th>
				<th>Max ms</th>
				<th>Dropped</th>
			</tr>
		</thead>
		<tbody>
			{results.map((r) => (
				<tr key={r.phase}>
					<td>{r.phase}</td>
					<td>{r.frames}</td>
					<td>
						{formatNumber(r.p50, {
							minimumFractionDigits: 1,
							maximumFractionDigits: 1,
						})}
					</td>
					<td>
						{formatNumber(r.p95, {
							minimumFractionDigits: 1,
							maximumFractionDigits: 1,
						})}
					</td>
					<td>
						{formatNumber(r.max, {
							minimumFractionDigits: 1,
							maximumFractionDigits: 1,
						})}
					</td>
					<td>{r.dropped}</td>
				</tr>
			))}
		</tbody>
	</table>
);

const Preview = ({ bench }: { bench: boolean }) => {
	const params = new URLSearchParams(location.search);
	const ids = bench ? BENCH : (params.get("agents")?.split(",") ?? PREVIEW);
	const [results, setResults] = useState<readonly BenchResult[] | null>(null);

	useEffect(() => {
		if (!bench) return;
		let stopped = false;
		void runBench(params.get("report"), Number(params.get("runs") ?? 1)).then((r) => {
			if (!stopped) setResults(r);
		});
		return () => {
			stopped = true;
		};
		// The page runs it once.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, []);

	const columns = Math.min(ids.length, 4);
	const rows = Math.ceil(ids.length / columns);
	return (
		<div
			className="relative grid h-full gap-px bg-border"
			style={{
				gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
				gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))`,
			}}
		>
			{ids.map((id) => (
				<Pane key={id} id={id} />
			))}
			{results ? <Results results={results} /> : null}
		</div>
	);
};

const source = previewSource();
const bench = new URLSearchParams(location.search).has("bench");
if (bench) reportErrors(new URLSearchParams(location.search).get("report"));

createRoot(document.getElementById("root")!).render(
	<StrictMode>
		<ServicesProvider source={source}>
			<Preview bench={bench} />
		</ServicesProvider>
	</StrictMode>
);
