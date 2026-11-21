// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One agent's row in the projects view: the activity dot, the title, its
// PR chips when it has a PR linked, its latest workflow run's chip, a few
// words on the right, or a muted Asleep once its idle CLI stopped, a cloud
// while an inbox change waits for the device, and the connection dot. A click
// opens it, and Option opens it beside the focused pane. Command adds it to
// the selection or takes it out, and Shift selects every row from the last
// one picked. With focus on it, Enter opens it and Up and Down move to the
// rows around it. A right-click, Shift+F10 or the ⋯ that shows on hover opens
// its inbox menu, and dragging drops it
// among the panes, or among the pins when it's pinned. A quote dragged
// onto it goes to its composer. A double-click on its title, or Rename in
// its menu, turns the title into a field. The agents it started through
// the agent tools sit indented under it, a faint line beside them for each
// level. More than two collapse into one summary row, which a click, Enter
// or Space opens and closes. While it's closed, the ones that need input,
// have failed, are open or are picked keep their own rows.

import { useI18n } from "@maxhq/shared/i18n";

import { AlarmClock, Check, CloudUpload } from "lucide-react";
import { type MouseEvent, useState } from "react";

import { openAgent } from "@/app/open-agent";
import { renamesOf, useShownTitle } from "@/app/rename";
import { useServices, useUi } from "@/app/services";
import { useQuoteDrop } from "@/composer/quote-drop";
import { RenameInput } from "@/components/rename-input";
import { AgentActivityDot, AgentConnectionDot } from "@/components/status/agent-dots";
import { holdsCommand } from "@/layout/shortcuts";
import { cn } from "@/lib/utils";
import { SmallPullChips } from "@/pulls/chip";
import { RowRunChipFor } from "@/workflow/containers";
import type { Agent, Timestamp } from "@maxhq/shared/bindings";
import { localLoading } from "@maxhq/shared/local-models/running";
import { loadingText } from "@maxhq/shared/local-models/wording";
import { agentLook, agentRowMeta } from "@maxhq/shared/status/activity";
import { showFailure } from "@maxhq/shared/ui/inbox-notice";
import { renameAgent } from "@maxhq/shared/ui/rename-agent";
import { foldKey, shownRows } from "@maxhq/shared/ui/rows";
import { inboxOf, isSnoozed } from "@maxhq/shared/view/inbox";
import type { ViewState } from "@maxhq/shared/view/replica";
import { slideLead } from "@maxhq/shared/ui/store";
import { foldStarted, startedOpenKey, startedRows, waitingFor } from "@maxhq/shared/view/select";
import { snoozeWakeLabel } from "@maxhq/shared/view/snooze";

import { pressAgent } from "./drag";
import { rowsFilter, useFilterFold } from "./filter";
import { RowMenu } from "./inbox-menu";
import { noticesOf } from "./notices";
import { prefsOf } from "./prefs";

/** A chevron that points down, or right when `closed`. */
export const Chevron = ({ closed, size = 10 }: { closed?: boolean; size?: number }) => {
	useI18n();
	return (
		<svg
			aria-hidden
			width={size}
			height={size}
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			strokeWidth={2.5}
			strokeLinecap="round"
			strokeLinejoin="round"
			className="shrink-0"
		>
			<path d={closed ? "M9 6l6 6-6 6" : "M6 9l6 6 6-6"} />
		</svg>
	);
};

const STRONG = new Set(["busy", "busy_in_subagents", "needs_input", "done_unread"]);

/** Whether a row shows as the one you're looking at. A lead does while you look at one of its sub-agents. */
export const useLookingAt = (): ((agent: Agent) => boolean) => {
	const selected = useUi((s) => s.selectedAgent);
	const lead = useUi(slideLead);
	return (agent) => selected === agent.id || lead === agent.id;
};

/** How far each level of started agents sits in from its caller. */
const LEVEL_INDENT = 14;

/** Where the first level's guide line runs: under the middle of the caller's dot. */
const GUIDE_AT = 25;

const ROW = "relative flex h-6 w-full shrink-0 items-center gap-2 rounded-md pr-2 pl-[22px]";

/** One agent in a project group, the pins or a shelf, and the agents it started. */
export const AgentRow = (props: {
	agent: Agent;
	view: ViewState;
	now: Timestamp;
	selected: boolean;
	/** Whether it sits among the pins, where dragging reorders them. */
	pinned?: boolean;
}) => {
	useI18n();

	const lookingAt = useLookingAt();
	const picked = useUi((s) => s.picked);
	const openKey = startedOpenKey(props.agent.id);
	const stored = useUi((s) => s.collapsed.has(openKey));
	const toggleFold = useUi((s) => s.toggleProject);
	const [open, toggle] = useFilterFold(foldKey("started", props.agent.id), stored, () =>
		toggleFold(openKey)
	);
	const fold = foldStarted(
		startedRows(props.view, props.agent.id, props.now),
		open,
		(agent) => lookingAt(agent) || picked.has(agent.id)
	);
	return (
		<>
			<Row {...props} />
			{fold.summary ? (
				<button
					type="button"
					aria-expanded={open}
					data-started-row={props.agent.id}
					onClick={toggle}
					style={{ paddingLeft: 22 + LEVEL_INDENT }}
					className="relative flex h-[22px] w-full shrink-0 items-center gap-1.5 rounded-md pr-2 text-left text-[11.5px] text-muted-foreground hover:text-secondary-foreground focus-visible:-outline-offset-2"
				>
					<Guides level={1} />
					<Chevron closed={!open} />
					<span className="min-w-0 flex-1 truncate">{fold.summary.text}</span>
				</button>
			) : null}
			{fold.rows.map(({ agent, level }) => (
				<Row
					key={agent.id}
					agent={agent}
					view={props.view}
					now={props.now}
					selected={lookingAt(agent)}
					level={level}
				/>
			))}
		</>
	);
};

/**
 * The faint lines beside a started row, one for each level it sits below
 * its caller, under the dot of the row each level hangs from.
 */
const Guides = ({ level }: { level: number }) =>
	Array.from({ length: level }, (_, i) => (
		<span
			// One line per level, which never reorder.
			key={i}
			aria-hidden
			className="pointer-events-none absolute -top-px bottom-0 w-px bg-border"
			style={{ left: GUIDE_AT + i * LEVEL_INDENT }}
		/>
	));

const Row = ({
	agent,
	view,
	now,
	selected,
	pinned = false,
	level = 0,
}: {
	agent: Agent;
	view: ViewState;
	now: Timestamp;
	selected: boolean;
	pinned?: boolean;
	level?: number;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const picked = useUi((s) => s.picked.has(agent.id));
	const look = agentLook(agent, now);
	const meta = agentRowMeta(view, agent, now);
	const loading = localLoading(view, agent);
	const inbox = inboxOf(agent);
	const snoozedUntil = isSnoozed(inbox, now) ? inbox.snoozed_until : null;
	const woke = inbox.woke_at !== null && snoozedUntil === null && inbox.settled === null;
	const waiting = waitingFor(view, agent.id);
	// A quote dragged from a timeline onto the row goes to its composer.
	const quoteDrop = useQuoteDrop<HTMLButtonElement>(
		agent.control === "observed" ? null : agent.id
	);
	const title = useShownTitle(agent);
	const [renaming, setRenaming] = useState(false);
	const indent = level > 0 ? { paddingLeft: 22 + level * LEVEL_INDENT } : undefined;

	const click = (e: MouseEvent) => {
		const ui = services.ui.getState();
		if (e.shiftKey) {
			const { workingShelf } = prefsOf(services).getState();
			ui.pickTo(
				agent.id,
				shownRows(
					services.view.getState().view,
					now,
					{ workingShelf },
					ui,
					rowsFilter(services)
				)
			);
		} else if (holdsCommand(e)) {
			ui.togglePicked(agent.id);
		} else {
			openAgent(services, agent.id, { split: e.altKey });
		}
	};
	const rename = (draft: string) => {
		setRenaming(false);
		void renameAgent(renamesOf(services), agent, draft).then((failure) => {
			if (failure !== null) showFailure(noticesOf(services), failure);
		});
	};

	if (renaming)
		return (
			<div style={indent} className={cn(ROW, "bg-accent")}>
				<Guides level={level} />
				<AgentActivityDot agent={agent} view={view} now={now} />
				<RenameInput
					title={title}
					label={t("Agent name")}
					onCommit={rename}
					onCancel={() => setRenaming(false)}
					className="h-5 text-[12.5px]"
				/>
			</div>
		);

	return (
		<RowMenu agent={agent.id} now={now} selected={selected} onRename={() => setRenaming(true)}>
			<button
				ref={quoteDrop}
				type="button"
				// An option in the sidebar's list, picked or not.
				role="option"
				aria-selected={picked}
				aria-current={selected ? "true" : undefined}
				data-picked={picked ? "" : undefined}
				data-pin-row={pinned ? agent.id : undefined}
				data-agent-row={agent.id}
				onClick={click}
				// Shift-clicking a range would select the text between the rows.
				onMouseDown={(e) => {
					if (e.shiftKey) e.preventDefault();
				}}
				onPointerDown={(e) => pressAgent(services, agent, e.nativeEvent, pinned)}
				style={indent}
				className={cn(
					ROW,
					// The ring sits inside, as the list's edges would clip it.
					"text-left focus-visible:-outline-offset-2 data-drop-over:shadow-[inset_0_0_0_1px_var(--pill-quote-border)] data-drop-over:bg-accent",
					// The selected row leaves room at its end for the menu's ⋯.
					selected
						? "bg-selected pr-7 shadow-[inset_0_0_0_1px_var(--selected-ring)]"
						: picked
							? "bg-accent"
							: "group-hover/agent-row:bg-accent"
				)}
			>
				<Guides level={level} />
				{picked ? (
					<Check
						aria-hidden
						size={11}
						strokeWidth={2.8}
						className="absolute text-primary"
						style={{ left: 6 + level * LEVEL_INDENT }}
					/>
				) : null}
				<AgentActivityDot agent={agent} view={view} now={now} />
				<span
					title={title}
					// Modified clicks are left to the selection.
					onDoubleClick={(e) => {
						if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
						setRenaming(true);
					}}
					// The chips give way before the title drops below about 40% of the row.
					className={cn(
						"min-w-[40%] flex-1 truncate text-[12.5px]",
						selected || woke || STRONG.has(look)
							? "text-foreground-strong"
							: "text-terminal",
						look === "done_unread" && "font-[650]"
					)}
				>
					{title}
				</span>
				<SmallPullChips agent={agent} />
				<RowRunChipFor agent={agent.id} />
				{meta.kind === "input" ? (
					<span className="inline-flex h-4 shrink-0 items-center rounded-[4px] bg-status-ready/16 px-[5px] text-[10.5px] font-semibold text-status-ready-text">
						{t("Input")}
					</span>
				) : woke ? (
					// Back from a snooze, until you open it.
					<span className="inline-flex shrink-0 items-center gap-1 text-[11px] font-medium text-status-connecting-text">
						<AlarmClock aria-hidden size={11} strokeWidth={2.2} />
						{t("Woke")}
					</span>
				) : snoozedUntil !== null ? (
					<span className="inline-flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground tabular-nums">
						<AlarmClock aria-hidden size={11} strokeWidth={2.2} />
						{snoozeWakeLabel(snoozedUntil, now)}
					</span>
				) : loading ? (
					// A local agent's turn waits while its model loads into memory.
					<span
						data-loading-model
						className="shrink-0 text-[11px] text-status-connecting-text tabular-nums"
					>
						{loadingText(loading.percent)}
					</span>
				) : meta.kind === "asleep" ? (
					<span className="shrink-0 text-[11px] text-subtle-foreground">{meta.text}</span>
				) : (
					<span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">
						{meta.text}
					</span>
				)}
				{waiting ? (
					// The change shows already, but the device hasn't taken it.
					<span
						title={t("Waiting for {{name}}", { name: waiting.name })}
						className="shrink-0 text-muted-foreground"
					>
						<CloudUpload aria-hidden size={11} strokeWidth={2.2} />
						<span className="sr-only">
							{t(", waiting for {{name}}", { name: waiting.name })}
						</span>
					</span>
				) : null}
				<AgentConnectionDot agent={agent} view={view} now={now} />
			</button>
		</RowMenu>
	);
};
