// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A workflow run's pane: its name and progress, a switch between the graph
// and the timeline, and the one picked. The graph waits on its layout,
// which loads ELK the first time any canvas needs it.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { useState } from "react";

import { useView } from "@/app/services";
import { useRunHeader, useRunLoad, useRunLoops, useRunNodes, useRunShape } from "@/app/use-run";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { Empty } from "@/main-area/empty";
import type { AgentId, LoopState, NodeDetail, RunKey, RunShape } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { runAt } from "@maxhq/shared/workflow/select";

import { useNodeDetail, useOpenNodeAgent } from "./containers";
import { useCanvasLayout } from "./elk";
import { LOOP_TONE } from "./node-card";
import { RunCanvas } from "./run-canvas";
import { RunTimeline } from "./run-timeline";

type Mode = "graph" | "timeline";

const MODES: readonly { mode: Mode; label: string }[] = [
	{
		mode: "graph",
		get label() {
			return t("Graph");
		},
	},
	{
		mode: "timeline",
		get label() {
			return t("Timeline");
		},
	},
];

type Acts = {
	readonly run: RunKey;
	readonly orchestrator: AgentId | null;
	readonly onOpenAgent: (agent: AgentId, split: boolean) => void;
	readonly onDetail: (node: string) => Promise<NodeDetail | null>;
};

const Graph = ({
	shape,
	loops,
	possible,
	...acts
}: Acts & { shape: RunShape; loops: readonly LoopState[]; possible: boolean }) => {
	const { t } = useI18n();

	const layout = useCanvasLayout(shape);
	if (!layout) return <Empty>{t("Laying out the run")}</Empty>;
	return <RunCanvas {...acts} shape={shape} layout={layout} loops={loops} possible={possible} />;
};

const Timeline = ({ run, shape, onOpenAgent }: Acts & { shape: RunShape }) => {
	useI18n();

	const nodes = useRunNodes(run);
	const now = useNow(5000);
	return <RunTimeline shape={shape} nodes={nodes} now={now} onOpenAgent={onOpenAgent} />;
};

/** The canvas pane for a run, from its store. */
export const CanvasPaneFor = ({ run }: { run: RunKey }) => {
	const { t } = useI18n();

	const entity = useView((s) => runAt(s.view, run));
	const project = useView((s) => s.view.tables.project.get(run.project)?.name);
	const shape = useRunShape(run);
	const header = useRunHeader(run);
	const load = useRunLoad(run);
	const loops = useRunLoops(run);
	const lead = entity?.orchestrators[0];
	const onOpenAgent = useOpenNodeAgent(lead);
	const onDetail = useNodeDetail(run);
	const [mode, setMode] = useState<Mode>("graph");
	const [possible, setPossible] = useState(false);
	const progress = header?.progress ?? entity?.progress;
	const acts: Acts = {
		run,
		orchestrator: entity?.orchestrators.at(-1) ?? null,
		onOpenAgent,
		onDetail,
	};

	return (
		<div className="flex h-full min-h-0 flex-col">
			<header className="flex shrink-0 items-center gap-3 border-b border-track px-3 py-2">
				<div className="flex min-w-0 flex-1 flex-col">
					<span className="truncate text-[13px] font-semibold text-foreground">
						{header?.label ?? entity?.label ?? run.run}
					</span>
					<span className="truncate text-[11px] text-muted-foreground">
						{project ?? run.workflow}
					</span>
				</div>
				{progress ? (
					<span
						role="img"
						aria-label={t("{{done}} of {{total}} done", {
							done: formatNumber(progress.done),
							total: formatNumber(progress.total),
						})}
						className="shrink-0 text-[11.5px] text-secondary-foreground tabular-nums"
					>
						{formatNumber(progress.done)}/{formatNumber(progress.total)}
					</span>
				) : null}
				{mode === "graph" && loops.some((loop) => loop.count === 0) ? (
					<label className="flex shrink-0 items-center gap-1.5 text-[11.5px] text-secondary-foreground">
						<Switch
							aria-label={t("Show possible loops")}
							checked={possible}
							onCheckedChange={setPossible}
						/>
						{t("Show possible loops")}
					</label>
				) : null}
				<div
					role="radiogroup"
					aria-label={t("Show the run as")}
					className="flex shrink-0 rounded-md border border-track p-px"
				>
					{MODES.map((each) => (
						<button
							key={each.mode}
							type="button"
							role="radio"
							aria-checked={mode === each.mode}
							onClick={() => setMode(each.mode)}
							className={cn(
								"h-[22px] rounded-[5px] px-2 text-[11.5px]",
								mode === each.mode
									? "bg-accent text-foreground"
									: "text-muted-foreground hover:text-foreground"
							)}
						>
							{each.label}
						</button>
					))}
				</div>
			</header>
			{load.notice ? (
				<p
					role="status"
					className={cn(
						"shrink-0 px-3 pt-1.5 text-[11px]",
						LOOP_TONE[load.notice.tone].text
					)}
				>
					{load.notice.text}
				</p>
			) : null}
			<div className="relative flex min-h-0 flex-1 flex-col">
				{shape ? (
					mode === "graph" ? (
						<Graph {...acts} shape={shape} loops={loops} possible={possible} />
					) : (
						<Timeline {...acts} shape={shape} />
					)
				) : load.status === "failed" ? (
					<Empty>
						{t("Couldn't open the run{{value1}}", {
							value1: load.error ? `: ${load.error}` : "",
						})}
					</Empty>
				) : (
					<Empty>{t("Loading the run")}</Empty>
				)}
			</div>
		</div>
	);
};
