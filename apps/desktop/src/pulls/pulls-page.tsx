// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's Pulls page: the open and closed PRs on two tabs, sorted by merge
// readiness unless you pick another sort. Each row opens its PR in the viewer
// beside the page, and each agent linked to it opens that agent.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Menu } from "@base-ui/react/menu";
import { Tabs } from "@base-ui/react/tabs";
import { ArrowLeft, ArrowUpDown, Check } from "lucide-react";
import { useMemo, useState } from "react";

import { openAgent } from "@/app/open-agent";
import { useServices, useUi, useView } from "@/app/services";
import { MENU_ITEM, MENU_POPUP } from "@/composer";
import { cn } from "@/lib/utils";
import { SECONDARY } from "@/settings/controls";
import { Badge } from "@/tasks/activity";
import type { ProjectId, PullListState, PullSummary } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { pullDevice, pullsDevice } from "@maxhq/shared/pulls/devices";
import { agentsWithPull, samePull } from "@maxhq/shared/pulls/links";
import { type PullSort, SORT_OPTIONS, sortPulls } from "@maxhq/shared/pulls/list";
import { formatAgoShort } from "@maxhq/shared/status/format";

import { ChecksMark, PullSize, ReviewMark, UnknownChecksBanner } from "./look";
import { usePulls } from "./queries";

const TAB =
	"h-[28px] rounded-[6px] px-2.5 text-[12.5px] text-secondary-foreground hover:bg-secondary data-active:bg-selected data-active:text-foreground";

const Line = ({ children, error }: { children: string; error?: boolean }) => {
	useI18n();
	return (
		<p
			role={error ? "alert" : undefined}
			className={cn(
				"px-1 py-8 text-center text-[12.5px]",
				error ? "text-status-error-text" : "text-muted-foreground"
			)}
		>
			{children}
		</p>
	);
};

const PullRow = ({ row, project }: { row: PullSummary; project: ProjectId }) => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const openPull = useUi((s) => s.openPull);
	const shown = useUi((s) => s.shownPull);
	const agents = useMemo(() => agentsWithPull(view, row.key), [view, row.key]);
	const device = pullDevice(view, row, project, now);
	const { pull } = row;
	return (
		<li className="flex items-center gap-2 border-t border-sidebar-border first:border-t-0">
			<button
				type="button"
				disabled={!device}
				onClick={() => device && openPull({ key: row.key, device })}
				aria-current={shown && samePull(shown.key, row.key) ? "true" : undefined}
				className="flex min-w-0 flex-1 flex-col gap-1 rounded-md px-2 py-2 text-left hover:bg-secondary aria-current:bg-selected"
			>
				<span className="flex min-w-0 items-center gap-2">
					<span className="shrink-0 font-mono text-xs text-muted-foreground tabular-nums">
						#{formatNumber(row.key.number, { useGrouping: false })}
					</span>
					<span className="min-w-0 truncate text-[13px] text-foreground-strong">
						{pull.title}
					</span>
					{pull.draft ? <Badge tone="plain">{t("Draft")}</Badge> : null}
					<span className="ml-auto flex shrink-0 items-center gap-2">
						<ChecksMark state={pull.checks} />
						<ReviewMark state={pull.review} />
						<PullSize pull={pull} />
						<span
							title={t("Last changed")}
							className="w-7 text-right text-[11px] text-muted-foreground tabular-nums"
						>
							{formatAgoShort(pull.updated_at, now)}
						</span>
					</span>
				</span>
				<span className="truncate text-[11.5px] text-muted-foreground">{pull.author}</span>
			</button>
			{agents.length > 0 ? (
				<span className="flex max-w-[40%] shrink-0 flex-wrap justify-end gap-1 pr-2">
					{agents.map((agent) => (
						<button
							key={agent.id}
							type="button"
							title={t("Open this agent")}
							onClick={() => openAgent(services, agent.id)}
							className="max-w-[180px] truncate rounded-[5px] bg-secondary px-1.5 py-0.5 text-[11px] text-secondary-foreground hover:bg-accent hover:text-foreground"
						>
							{agent.title}
						</button>
					))}
				</span>
			) : null}
		</li>
	);
};

const PullList = ({
	project,
	state,
	sort,
}: {
	project: ProjectId;
	state: PullListState;
	sort: PullSort;
}) => {
	const { t } = useI18n();

	const list = usePulls(project, state);
	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const rows = useMemo(() => (list.data ? sortPulls(list.data, sort) : []), [list.data, sort]);
	if (list.error)
		return (
			<Line error>
				{t("Couldn't read the pull requests. {{message}}", { message: list.error.message })}
			</Line>
		);
	if (!list.data) return <Line>{t("Reading the pull requests")}</Line>;
	if (rows.length === 0) return <Line>{t("No {{state}} pull requests", { state: state })}</Line>;
	const unknown = rows.find((row) => row.pull.checks === "unknown");
	return (
		<div className="flex flex-col gap-3">
			{unknown ? (
				<UnknownChecksBanner
					repo={unknown.key.repo}
					device={pullsDevice(view, project, now)}
				/>
			) : null}
			<ul
				aria-label={t("{{value1}} pull requests", {
					value1: state === "open" ? t("Open") : t("Closed"),
				})}
			>
				{rows.map((row) => (
					<PullRow
						key={`${row.key.repo}#${row.key.number}`}
						row={row}
						project={project}
					/>
				))}
			</ul>
		</div>
	);
};

const SortMenu = ({ sort, onSort }: { sort: PullSort; onSort: (sort: PullSort) => void }) => {
	const { t } = useI18n();

	const label = SORT_OPTIONS.find((option) => option.value === sort)?.label ?? "";
	return (
		<Menu.Root>
			<Menu.Trigger aria-label={t("Sort: {{label}}", { label: label })} className={SECONDARY}>
				<ArrowUpDown aria-hidden size={13} />
				{label}
			</Menu.Trigger>
			<Menu.Portal>
				<Menu.Positioner side="bottom" align="end" sideOffset={4} className="z-[130]">
					<Menu.Popup className={cn(MENU_POPUP, "w-[220px]")}>
						<Menu.RadioGroup
							value={sort}
							onValueChange={(value: PullSort) => onSort(value)}
						>
							{SORT_OPTIONS.map((option) => (
								<Menu.RadioItem
									key={option.value}
									value={option.value}
									className={MENU_ITEM}
								>
									<span className="flex-1 text-[12.5px] font-[550]">
										{option.label}
									</span>
									<span className="flex size-3.5 items-center justify-center">
										<Menu.RadioItemIndicator>
											<Check
												aria-hidden
												className="size-3.5 text-primary"
												strokeWidth={2.6}
											/>
										</Menu.RadioItemIndicator>
									</span>
								</Menu.RadioItem>
							))}
						</Menu.RadioGroup>
					</Menu.Popup>
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.Root>
	);
};

/** A project's Pulls page. */
export const PullsPage = ({ project }: { project: ProjectId }) => {
	const { t } = useI18n();

	const name = useView((s) => s.view.tables.project.get(project)?.name ?? t("Project"));
	const closePulls = useUi((s) => s.closePulls);
	const [sort, setSort] = useState<PullSort>("ready");
	return (
		<Tabs.Root defaultValue="open" className="flex h-full flex-col">
			<header className="flex flex-col gap-3 border-b border-sidebar-border px-5 pt-4 pb-3">
				<div className="flex items-center gap-3">
					<button
						type="button"
						onClick={closePulls}
						className={SECONDARY}
						title={t("Back to the agents")}
					>
						<ArrowLeft aria-hidden size={13} />
						{t("Back")}
					</button>
					<div className="min-w-0 flex-1">
						<p className="truncate text-[11.5px] text-muted-foreground">{name}</p>
						<h1 className="text-[15px] font-[650] text-foreground-strong">
							{t("Pull requests")}
						</h1>
					</div>
				</div>
				<div className="flex items-center gap-2">
					<Tabs.List className="flex flex-1 gap-1">
						<Tabs.Tab value="open" className={TAB}>
							{t("Open")}
						</Tabs.Tab>
						<Tabs.Tab value="closed" className={TAB}>
							{t("Closed")}
						</Tabs.Tab>
					</Tabs.List>
					<SortMenu sort={sort} onSort={setSort} />
				</div>
			</header>
			<div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
				<Tabs.Panel value="open">
					<PullList project={project} state="open" sort={sort} />
				</Tabs.Panel>
				<Tabs.Panel value="closed">
					<PullList project={project} state="closed" sort={sort} />
				</Tabs.Panel>
			</div>
		</Tabs.Root>
	);
};
