// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The bell in the header and the inbox it opens. The bell counts what the
// Dock badge counts, and Now lists exactly those agents. With Show the badge
// off the bell shows no count, though Now still lists them. Earlier is the log
// of past notifications from every device, newest first. Opening the
// popover marks nothing read; opening an agent does, as Mark all as read
// does for every notification.

import { formatNumber, t, useI18n } from "@maxhq/shared/i18n";

import { useMemo, useState } from "react";
import { useStore } from "zustand";

import { askForRequest } from "@/agent-view/request";
import { openAgent } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { Popover, PopoverPopup, PopoverTitle, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { useBadgePrefs } from "@/native/dock-badge";
import { openSettings } from "@/settings/store";
import type { Timestamp } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	bellCount,
	createNoticeLog,
	earlierRows,
	finishedAgents,
	hasMore,
	type InboxRow,
	nowRows,
} from "@maxhq/shared/notices/inbox";
import { formatAgoShort } from "@maxhq/shared/status/format";

/** What the bell's screen reader label says, with no count while the badge is off. */
export const bellLabel = (count: number | null): string => {
	if (count === null) return t("Notifications");
	if (count === 0) return t("No agent needs you");
	return t("{{countText}} agents need you", { count, countText: formatNumber(count) });
};

const BellIcon = () => (
	<svg
		aria-hidden
		width={17}
		height={17}
		viewBox="0 0 24 24"
		fill="none"
		stroke="currentColor"
		strokeWidth={1.8}
		strokeLinecap="round"
		strokeLinejoin="round"
	>
		<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0" />
	</svg>
);

const TEXT_BUTTON =
	"rounded-[5px] px-1.5 py-0.5 text-[11.5px] text-muted-foreground enabled:hover:bg-accent enabled:hover:text-foreground disabled:opacity-50";

/** One row: the agent, where it runs, what it wants and when. */
const Row = ({ row, now, onOpen }: { row: InboxRow; now: Timestamp; onOpen: () => void }) => (
	<li>
		<button
			type="button"
			aria-disabled={row.gone || undefined}
			onClick={row.gone ? undefined : onOpen}
			className={cn(
				"grid w-full grid-cols-[10px_minmax(0,1fr)_auto] items-baseline gap-x-1.5 px-3.5 py-[7px] text-left outline-none focus-visible:bg-accent",
				row.gone ? "cursor-default" : "hover:bg-accent"
			)}
		>
			<span
				aria-hidden
				className={cn("size-1.5 self-center rounded-full", row.unread && "bg-status-ready")}
			/>
			<span className="truncate text-[12.5px] font-medium text-foreground">{row.title}</span>
			<span className="text-[11px] text-muted-foreground tabular-nums">
				{formatAgoShort(row.at, now)}
			</span>
			<span className="col-start-2 col-end-4 truncate text-[11.5px] text-secondary-foreground">
				{row.place} · {row.what}
			</span>
			{row.stays || row.gone ? (
				<span className="col-start-2 col-end-4 text-[11px] text-muted-foreground">
					{row.gone ? t("This agent is gone") : row.stays}
				</span>
			) : null}
		</button>
	</li>
);

/** A section's heading, rows and empty state. */
const Section = ({
	title,
	rows,
	empty,
	now,
	onOpen,
}: {
	title: string;
	rows: readonly InboxRow[];
	empty: string;
	now: Timestamp;
	onOpen: (row: InboxRow) => void;
}) => (
	<section aria-label={title} className="py-1">
		<h3 className="px-3.5 pt-1.5 pb-1 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">
			{title}
		</h3>
		{rows.length === 0 ? (
			<p className="px-3.5 pb-1.5 text-[11.5px] text-muted-foreground">{empty}</p>
		) : (
			<ul>
				{rows.map((row) => (
					<Row key={row.key} row={row} now={now} onOpen={() => onOpen(row)} />
				))}
			</ul>
		)}
	</section>
);

/** The bell, which opens the inbox. */
export const NotificationBell = () => {
	useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const prefs = useBadgePrefs((p) => p);
	const { counts } = prefs;
	const now = useNow(30_000);
	const log = useMemo(() => createNoticeLog(services.source), [services.source]);
	const { page, failed } = useStore(log.store);
	const [open, setOpen] = useState(false);

	const current = useMemo(() => nowRows(view, now, counts), [view, now, counts]);
	const earlier = useMemo(
		() => earlierRows(view, page?.notices ?? [], now, counts),
		[view, page, now, counts]
	);
	const count = bellCount(current, prefs);

	const onOpenChange = (next: boolean) => {
		setOpen(next);
		if (next) void log.load();
	};
	const openRow = (row: InboxRow) => {
		setOpen(false);
		openAgent(services, row.agent);
		const agent = view.tables.agent.get(row.agent);
		if (agent?.activity.state === "needs_input")
			askForRequest(agent.id, agent.activity.attention);
	};
	const finished = finishedAgents(view, now, counts);
	const unread = (page?.unread ?? 0) > 0 || finished.length > 0;

	return (
		<Popover open={open} onOpenChange={onOpenChange}>
			<PopoverTrigger
				aria-label={bellLabel(count)}
				title={t("Notifications")}
				className="relative grid size-[30px] place-items-center rounded-[7px] text-secondary-foreground hover:bg-secondary data-popup-open:bg-secondary"
			>
				<BellIcon />
				{count ? (
					<span
						aria-hidden
						className="absolute top-[3px] right-0.5 h-[15px] min-w-[15px] rounded-lg bg-status-ready px-1 text-center text-[10px] leading-[15px] font-bold text-status-ready-foreground"
					>
						{formatNumber(count)}
					</span>
				) : null}
			</PopoverTrigger>
			<PopoverPopup
				side="bottom"
				align="end"
				sideOffset={6}
				className="flex max-h-[min(560px,var(--available-height))] w-[380px] flex-col overflow-hidden rounded-xl border-input shadow-large"
			>
				<div className="flex items-center gap-1 border-b border-track px-3.5 pt-3 pb-2">
					<PopoverTitle className="mr-auto text-[13.5px] leading-tight font-[650] text-foreground">
						{t("Notifications")}
					</PopoverTitle>
					<button
						type="button"
						disabled={!unread}
						onClick={() => void log.readAll(finished)}
						className={TEXT_BUTTON}
					>
						{t("Mark all as read")}
					</button>
					<button
						type="button"
						disabled={earlier.length === 0}
						onClick={() => void log.clear()}
						className={TEXT_BUTTON}
					>
						{t("Clear")}
					</button>
				</div>
				<div className="min-h-0 overflow-y-auto">
					<Section
						title={t("Now")}
						rows={current}
						empty={t("Nothing needs you right now")}
						now={now}
						onOpen={openRow}
					/>
					<div className="mx-3.5 border-t border-track" />
					<Section
						title={t("Earlier")}
						rows={earlier}
						empty={page === null ? t("Loading…") : t("No notifications yet")}
						now={now}
						onOpen={openRow}
					/>
					{failed ? (
						<p role="alert" className="px-3.5 pb-2 text-[11px] text-status-error-text">
							{failed}
						</p>
					) : null}
					{page && hasMore(page) ? (
						<button
							type="button"
							onClick={() => void log.more()}
							className={cn(TEXT_BUTTON, "mx-2.5 mb-2")}
						>
							{t("Show more")}
						</button>
					) : null}
				</div>
				<div className="border-t border-track px-3.5 py-2">
					<button
						type="button"
						onClick={() => {
							setOpen(false);
							openSettings(services, { kind: "page", at: "notifications" });
						}}
						className="text-[11.5px] text-primary hover:underline"
					>
						{t("Notification settings")}
					</button>
				</div>
			</PopoverPopup>
		</Popover>
	);
};
