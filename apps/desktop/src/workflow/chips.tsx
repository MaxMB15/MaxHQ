// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A lead's latest run as a chip, such as "pipeline 11/18", on its sidebar
// row and in its header, and a project's Workflows entry, which folds, with
// a row per run. Each opens the run in the slide-out.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Workflow } from "lucide-react";
import type { MouseEvent, ReactNode } from "react";

import { cn } from "@/lib/utils";
import { Chevron } from "@/sidebar/agent-row";
import { RightClickMenu } from "@/sidebar/project-menu";
import type { Timestamp, WorkflowRun, WorkflowRunId } from "@maxhq/shared/bindings";
import { formatAgoShort } from "@maxhq/shared/status/format";
import { type RunChip, runStatus } from "@maxhq/shared/workflow/select";

import { NodeGlyph } from "./glyph";

const LOOK = (chip: RunChip) =>
	chip.needsYou
		? "border-status-busy/40 text-status-busy-text"
		: chip.done
			? "border-track text-muted-foreground"
			: "border-track text-secondary-foreground";

/**
 * The chip on a sidebar row. The row is a button already, so the chip is
 * a span that takes the click before the row does. It shrinks, cutting its
 * text, before the row's title does.
 */
export const RowRunChip = ({ chip, onOpen }: { chip: RunChip; onOpen: () => void }) => {
	useI18n();
	return (
		<span
			title={chip.label}
			onClick={(e: MouseEvent) => {
				if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
				e.stopPropagation();
				onOpen();
			}}
			className={cn(
				"inline-flex h-4 max-w-[84px] min-w-0 items-center rounded-[4px] border px-1 text-[10.5px] font-[600] tabular-nums hover:bg-accent",
				LOOK(chip)
			)}
		>
			<span aria-hidden className="truncate">
				{chip.text}
			</span>
			<span className="sr-only">, {chip.spoken}</span>
		</span>
	);
};

/** The chip in the agent header. */
export const HeaderRunChip = ({ chip, onOpen }: { chip: RunChip; onOpen: () => void }) => {
	const { t } = useI18n();
	return (
		<button
			type="button"
			title={chip.label}
			aria-label={t("Workflow, {{spoken}}", { spoken: chip.spoken })}
			onClick={onOpen}
			className={cn(
				"inline-flex h-[22px] max-w-[160px] shrink-0 items-center gap-[5px] rounded-md border bg-secondary px-[7px] text-[11.5px] whitespace-nowrap tabular-nums hover:bg-accent",
				LOOK(chip)
			)}
		>
			<Workflow aria-hidden size={12} strokeWidth={2} className="shrink-0" />
			<span className="truncate">{chip.text}</span>
		</button>
	);
};

const ROW =
	"flex h-6 w-full shrink-0 items-center gap-2 rounded-md pr-2 text-left text-[12.5px] text-secondary-foreground hover:bg-accent hover:text-foreground";

/** One run's row: its state, its name, and its progress while live or when it ended. */
const RunRow = ({
	run,
	active,
	now,
	onOpen,
}: {
	run: WorkflowRun;
	active: boolean;
	now: Timestamp;
	onOpen: (run: WorkflowRunId) => void;
}) => {
	useI18n();
	return (
		<button
			type="button"
			aria-current={active ? "true" : undefined}
			title={run.label}
			onClick={() => onOpen(run.id)}
			className={cn(ROW, "pl-[36px]", active && "bg-selected text-foreground")}
		>
			<NodeGlyph status={runStatus(run)} provisional={false} />
			<span className="min-w-0 flex-1 truncate">{run.label}</span>
			{run.done ? (
				run.last_activity !== null ? (
					<span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">
						{formatAgoShort(run.last_activity, now)}
					</span>
				) : null
			) : (
				<span className="shrink-0 text-[11px] text-secondary-foreground tabular-nums">
					{formatNumber(run.progress.done)}/{formatNumber(run.progress.total)}
				</span>
			)}
		</button>
	);
};

/**
 * A project's Workflows entry: a row that folds, with how many runs there
 * are, and a row per run under it, live ones first. While folded, the row
 * shows the state of the liveliest run. Nothing when there are no runs.
 */
export const WorkflowsEntry = ({
	project,
	runs,
	folded,
	showing,
	now,
	onToggle,
	onOpen,
	entryMenu,
	runMenu,
}: {
	/** The project's name, for screen readers. */
	project: string;
	/** The runs, in the order they show. */
	runs: readonly WorkflowRun[];
	folded: boolean;
	/** The run shown in the slide-out, if one of these. */
	showing: WorkflowRunId | null;
	now: Timestamp;
	onToggle: () => void;
	onOpen: (run: WorkflowRunId) => void;
	/** The entry row's right-click menu. */
	entryMenu: ReactNode;
	/** A run row's right-click menu. */
	runMenu: (run: WorkflowRun) => ReactNode;
}) => {
	const { t } = useI18n();

	if (runs.length === 0) return null;
	const liveliest = runs.find((run) => run.needs_you && !run.done) ?? runs[0];
	return (
		<div
			role="group"
			aria-label={t("Workflows in {{project}}", { project: project })}
			className="flex flex-col"
		>
			<RightClickMenu items={entryMenu}>
				<button
					type="button"
					aria-expanded={!folded}
					onClick={onToggle}
					className={cn(ROW, "pl-[22px]")}
				>
					<Workflow aria-hidden size={13} strokeWidth={2} className="shrink-0" />
					<span className="min-w-0 flex-1 truncate">{t("Workflows")}</span>
					{folded && liveliest && !liveliest.done ? (
						<NodeGlyph status={runStatus(liveliest)} provisional={false} />
					) : null}
					<span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">
						{formatNumber(runs.length)}
					</span>
					<Chevron closed={folded} />
				</button>
			</RightClickMenu>
			{folded
				? null
				: runs.map((run) => (
						<RightClickMenu key={run.id} items={runMenu(run)}>
							<RunRow
								run={run}
								active={showing === run.id}
								now={now}
								onOpen={onOpen}
							/>
						</RightClickMenu>
					))}
		</div>
	);
};
