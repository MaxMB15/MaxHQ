// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The inbox sections around the project groups: the pinned agents above
// them, in the order you dragged them into, and the shelves below, open or
// closed as you last left them. Working holds the busy agents while its
// setting is on, Snoozed the ones that come back later, and Settled the
// ones you're done with, a page at a time. A closed shelf still shows the
// agent you're looking at.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Activity, AlarmClock, CircleCheck, Pin } from "lucide-react";
import type { ReactNode } from "react";

import { useUi } from "@/app/services";
import type { Agent, Timestamp } from "@maxhq/shared/bindings";
import { foldKey, shelfRows } from "@maxhq/shared/ui/rows";
import { SETTLED_PAGE, type Shelf } from "@maxhq/shared/ui/store";
import type { ViewState } from "@maxhq/shared/view/replica";
import { type InboxSections, startedRows } from "@maxhq/shared/view/select";

import { AgentRow, Chevron, useLookingAt } from "./agent-row";
import { useFilterFold } from "./filter";

const ICON = { "aria-hidden": true, size: 11, strokeWidth: 2.2, className: "shrink-0" } as const;

/** The pinned agents, above every project. Hidden when none are pinned. */
export const PinnedList = ({
	agents,
	view,
	now,
}: {
	agents: readonly Agent[];
	view: ViewState;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const lookingAt = useLookingAt();
	if (agents.length === 0) return null;
	return (
		<section aria-label={t("Pinned")} className="mb-1.5 flex flex-col gap-px">
			<div className="flex h-[22px] shrink-0 items-center gap-1.5 px-2 text-xs font-semibold text-muted-foreground">
				<Pin {...ICON} />
				<span className="min-w-0 flex-1 truncate text-terminal">{t("Pinned")}</span>
			</div>
			{agents.map((agent) => (
				<AgentRow
					key={agent.id}
					agent={agent}
					view={view}
					now={now}
					selected={lookingAt(agent)}
					pinned
				/>
			))}
		</section>
	);
};

const ShelfSection = ({
	shelf,
	label,
	icon,
	agents,
	view,
	now,
}: {
	shelf: Shelf;
	label: string;
	icon: ReactNode;
	agents: readonly Agent[];
	view: ViewState;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const openShelves = useUi((s) => s.openShelves);
	const settledShown = useUi((s) => s.settledShown);
	const selectedAgent = useUi((s) => s.selectedAgent);
	const slideOut = useUi((s) => s.slideOut);
	const toggleShelf = useUi((s) => s.toggleShelf);
	const showMoreSettled = useUi((s) => s.showMoreSettled);
	const lookingAt = useLookingAt();
	const [open, toggle] = useFilterFold(foldKey("shelf", shelf), openShelves.has(shelf), () =>
		toggleShelf(shelf)
	);
	if (agents.length === 0) return null;
	const rows = shelfRows(
		shelf,
		agents,
		{
			openShelves: open ? new Set([shelf]) : new Set(),
			settledShown,
			selectedAgent,
			slideOut,
		},
		(agent) => startedRows(view, agent.id, now).map((row) => row.agent)
	);
	const hidden = open ? agents.length - rows.length : 0;
	return (
		<section aria-label={label} className="flex flex-col gap-px">
			<button
				type="button"
				aria-expanded={open}
				onClick={toggle}
				className="flex h-[22px] shrink-0 items-center gap-1.5 rounded-md px-2 text-left text-xs font-semibold text-muted-foreground hover:text-secondary-foreground"
			>
				<Chevron closed={!open} />
				{icon}
				<span className="min-w-0 flex-1 truncate">{label}</span>
				<span className="shrink-0 text-[11px] font-normal tabular-nums">
					{formatNumber(agents.length)}
				</span>
			</button>
			{rows.map((agent) => (
				<AgentRow
					key={agent.id}
					agent={agent}
					view={view}
					now={now}
					selected={lookingAt(agent)}
				/>
			))}
			{hidden > 0 ? (
				<button
					type="button"
					onClick={showMoreSettled}
					className="flex h-[22px] shrink-0 items-center rounded-md pr-2 pl-[22px] text-left text-[11.5px] text-muted-foreground hover:text-secondary-foreground"
				>
					{t("Show {{value2}} more", {
						value2: formatNumber(Math.min(hidden, SETTLED_PAGE)),
					})}
				</button>
			) : null}
		</section>
	);
};

/** The shelves at the bottom of the list, each hidden while it's empty. */
export const Shelves = ({
	sections,
	view,
	now,
}: {
	sections: InboxSections;
	view: ViewState;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const { working, snoozed, settled } = sections;
	if (working.length + snoozed.length + settled.length === 0) return null;
	return (
		<div className="mt-auto flex shrink-0 flex-col gap-px pt-2">
			<ShelfSection
				shelf="working"
				label={t("Working")}
				icon={<Activity {...ICON} />}
				agents={working}
				view={view}
				now={now}
			/>
			<ShelfSection
				shelf="snoozed"
				label={t("Snoozed")}
				icon={<AlarmClock {...ICON} />}
				agents={snoozed}
				view={view}
				now={now}
			/>
			<ShelfSection
				shelf="settled"
				label={t("Settled")}
				icon={<CircleCheck {...ICON} />}
				agents={settled}
				view={view}
				now={now}
			/>
		</div>
	);
};
