// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The panel that slides out right of the sidebar when an agent with
// sub-agents is open, or a workflow run is. Its Sub-agents tab lists the
// lead first, then each sub-agent with its dot and status, then the
// lead's context window. A lead with a run gets a Workflow tab too. A
// sub-agent card's "Show in panel" scrolls to that sub-agent's row and
// rings it.

import { t, useI18n } from "@maxhq/shared/i18n";

import { type CSSProperties, type ReactNode, type Ref, useCallback, useMemo } from "react";

import { useUi, useView } from "@/app/services";
import { AgentActivityDot } from "@/components/status/agent-dots";
import type { Agent, Timestamp } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";
import { subAgentStatus } from "@maxhq/shared/status/activity";
import { formatContext } from "@maxhq/shared/status/format";
import { contextShare, leadStatus } from "@maxhq/shared/status/sub-agents";
import type { ViewState } from "@maxhq/shared/view/replica";
import { spotOf } from "@maxhq/shared/ui/store";
import { childrenOf } from "@maxhq/shared/view/select";

const Row = ({
	agent,
	view,
	now,
	lead,
	selected,
	spotted = false,
	ref,
	onOpen,
}: {
	agent: Agent;
	view: ViewState;
	now: Timestamp;
	lead?: boolean;
	selected: boolean;
	/** Set on the row a sub-agent card asked the panel to show. */
	spotted?: boolean;
	ref?: Ref<HTMLButtonElement>;
	onOpen: () => void;
}) => {
	const { t } = useI18n();

	const status = lead ? leadStatus(agent.activity) : subAgentStatus(agent.activity);
	const quiet = agent.activity.state === "starting" && !lead;
	return (
		<button
			ref={ref}
			type="button"
			aria-current={selected ? "true" : undefined}
			data-spotted={spotted || undefined}
			onClick={onOpen}
			className={cn(
				"flex h-6 w-full shrink-0 items-center gap-2 rounded-md px-1.5 text-left",
				selected
					? "bg-selected shadow-[inset_0_0_0_1px_var(--selected-ring)]"
					: spotted
						? "bg-accent shadow-[inset_0_0_0_1px_var(--primary)]"
						: lead
							? "bg-secondary hover:bg-accent"
							: "hover:bg-accent"
			)}
		>
			<AgentActivityDot agent={agent} view={view} now={now} />
			<span
				className={cn(
					"min-w-0 flex-1 truncate text-xs",
					lead || selected || spotted
						? "text-foreground"
						: quiet
							? "text-muted-foreground"
							: "text-terminal"
				)}
			>
				{lead ? t("Lead") : agent.title}
			</span>
			<span className="shrink-0 text-[11px] text-muted-foreground">{status}</span>
		</button>
	);
};

/** The slide-out's width: wider whenever it shows a run, on either tab, so a tab switch never moves the main area. */
export const slideOutWidth = (hasRun: boolean): number => (hasRun ? 320 : 216);

/** The open lead's sub-agents, then the lead's context window: the Sub-agents tab. */
export const SubAgentList = ({ lead, now }: { lead: Agent; now: Timestamp }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const selected = useUi((s) => s.selectedAgent);
	const selectAgent = useUi((s) => s.selectAgent);
	const selectSubAgent = useUi((s) => s.selectSubAgent);
	const spot = useUi((s) => spotOf(s, lead.id));
	// Runs as the spotted row mounts or the spot moves, not on every render.
	const scrollTo = useCallback((row: HTMLButtonElement | null) => {
		row?.scrollIntoView({ block: "nearest" });
	}, []);
	const children = useMemo(() => childrenOf(view, lead.id), [view, lead.id]);
	const context = lead.context;
	const percent = contextShare(context);

	return (
		<>
			<Row
				agent={lead}
				view={view}
				now={now}
				lead
				selected={selected === lead.id}
				onOpen={() => selectAgent(lead.id, true)}
			/>
			{children.map((child) => (
				<Row
					key={child.id}
					agent={child}
					view={view}
					now={now}
					selected={selected === child.id}
					spotted={spot === child.id}
					ref={spot === child.id ? scrollTo : undefined}
					onOpen={() => selectSubAgent(child.id)}
				/>
			))}
			{context && percent !== null ? (
				<div className="mx-1.5 mt-3 flex flex-col gap-1.5 border-t border-sidebar-border pt-2.5 text-[11px]">
					<div className="flex items-center justify-between text-muted-foreground">
						<span>{t("Context")}</span>
						<span className="text-secondary-foreground tabular-nums">
							{formatContext(context.used_tokens, context.window_tokens)}
						</span>
					</div>
					<div
						role="meter"
						aria-label={t("Lead's context window")}
						aria-valuemin={0}
						aria-valuemax={100}
						aria-valuenow={Math.round(percent)}
						className="h-1 overflow-hidden rounded-[2px] bg-track"
					>
						<div
							className="h-full rounded-[2px] bg-primary"
							style={{ width: `${percent}%` }}
						/>
					</div>
				</div>
			) : null}
		</>
	);
};

const TAB_NAMES = {
	get agents() {
		return t("Sub-agents");
	},
	get workflow() {
		return t("Workflow");
	},
} as const;

/**
 * The slide-out right of the sidebar: a header with a close button, the
 * Sub-agents and Workflow tabs when `onTab` is given, and the tab's
 * content below.
 */
export const SlideOut = ({
	label,
	title,
	subtitle,
	width,
	closeLabel,
	onClose,
	tab,
	onTab,
	children,
}: {
	/** What a screen reader calls the panel. */
	label: string;
	title: string;
	subtitle: string;
	width: number;
	closeLabel: string;
	onClose: () => void;
	tab?: "agents" | "workflow";
	/** Shows the tabs, for a lead with a run. */
	onTab?: (tab: "agents" | "workflow") => void;
	children: ReactNode;
}) => {
	const { t } = useI18n();
	return (
		<aside
			aria-label={label}
			className="flex shrink-0 animate-slide-in flex-col gap-px overflow-y-auto border-r border-sidebar-border bg-subpanel px-2 py-2.5 shadow-side"
			style={{ width, "--dot-surface": "var(--subpanel)" } as CSSProperties}
		>
			<div className="flex items-start gap-1.5 px-1.5 pb-2">
				<div className="min-w-0 flex-1">
					<h2 className="truncate text-xs font-semibold text-foreground">{title}</h2>
					<p className="line-clamp-2 text-[11px] text-muted-foreground">{subtitle}</p>
				</div>
				<button
					type="button"
					aria-label={closeLabel}
					onClick={onClose}
					className="grid size-[22px] shrink-0 place-items-center rounded-[5px] text-muted-foreground hover:bg-accent hover:text-foreground"
				>
					<svg
						aria-hidden
						width={12}
						height={12}
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						strokeWidth={2.4}
						strokeLinecap="round"
						strokeLinejoin="round"
					>
						<path d="M15 6l-6 6 6 6" />
					</svg>
				</button>
			</div>
			{onTab ? (
				<div
					role="tablist"
					aria-label={t("{{title}} views", { title: title })}
					className="mb-1.5 flex gap-0.5 px-1"
				>
					{(["agents", "workflow"] as const).map((name) => (
						<button
							key={name}
							type="button"
							role="tab"
							aria-selected={tab === name}
							onClick={() => onTab(name)}
							className={cn(
								"h-6 flex-1 rounded-md text-[11.5px] font-[550]",
								tab === name
									? "bg-secondary text-foreground"
									: "text-muted-foreground hover:bg-accent hover:text-secondary-foreground"
							)}
						>
							{TAB_NAMES[name]}
						</button>
					))}
				</div>
			) : null}
			{onTab ? (
				<div
					role="tabpanel"
					aria-label={TAB_NAMES[tab ?? "agents"]}
					className="flex flex-col gap-px"
				>
					{children}
				</div>
			) : (
				children
			)}
		</aside>
	);
};
