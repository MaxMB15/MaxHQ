// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One dialog with a task's runs and the requests it took, on two tabs. Each
// run names the agent it reached, which opens it and closes the settings. A
// timed task takes no requests, so it opens on its runs.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { Tabs } from "@base-ui/react/tabs";
import { RefreshCw } from "lucide-react";

import { openAgent } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { cn } from "@/lib/utils";
import { SECONDARY } from "@/settings/controls";
import { closeSettings } from "@/settings/store";
import type { Run, Task, TaskDelivery } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { formatWhen } from "@maxhq/shared/status/format";
import { takesRequests } from "@maxhq/shared/tasks/draft";
import {
	deliveryFacts,
	deliveryLabel,
	deliveryTone,
	RUN_CAUSE,
	RUN_OUTCOME,
	runTone,
	type Tone,
} from "@maxhq/shared/tasks/format";

import { useDeliveries, useRefreshActivity, useRuns } from "./queries";

const TONE: Record<Tone, string> = {
	good: "bg-status-ready/15 text-status-ready-text",
	warn: "bg-status-busy/15 text-status-busy-text",
	bad: "bg-status-error/15 text-status-error-text",
	info: "bg-primary/15 text-primary",
	plain: "bg-secondary text-muted-foreground",
};

/** A short word with a tone behind it. */
export const Badge = ({ tone, children }: { tone: Tone; children: string }) => {
	useI18n();
	return (
		<span
			className={cn(
				"inline-flex h-[20px] shrink-0 items-center rounded-[5px] px-1.5 text-[11px] font-[600]",
				TONE[tone]
			)}
		>
			{children}
		</span>
	);
};

const Empty = ({ children }: { children: string }) => {
	useI18n();
	return <p className="px-1 py-6 text-center text-[12.5px] text-muted-foreground">{children}</p>;
};

const RunRow = ({ run, onLeave }: { run: Run; onLeave: () => void }) => {
	const { t } = useI18n();

	const services = useServices();
	const now = useNow(60_000);
	const agent = useView((s) => (run.agent ? s.view.tables.agent.get(run.agent) : undefined));
	return (
		<li className="flex items-center gap-3 border-t border-sidebar-border py-2.5 first:border-t-0">
			<Badge tone={runTone(run.outcome)}>{RUN_OUTCOME[run.outcome]}</Badge>
			<div className="flex min-w-0 flex-1 flex-col gap-0.5">
				<span className="text-[12.5px] text-foreground">
					{RUN_CAUSE[run.cause]}, {formatWhen(run.started, now)}
				</span>
				{run.error ? (
					<span className="text-xs text-status-error-text">{run.error}</span>
				) : null}
			</div>
			{run.agent ? (
				agent ? (
					<button
						type="button"
						className={SECONDARY}
						onClick={() => {
							onLeave();
							closeSettings(services);
							openAgent(services, agent.id);
						}}
					>
						{agent.title}
					</button>
				) : (
					<span className="text-xs text-muted-foreground">{t("Agent gone")}</span>
				)
			) : null}
		</li>
	);
};

const DeliveryRow = ({ delivery }: { delivery: TaskDelivery }) => {
	const { t } = useI18n();

	const now = useNow(60_000);
	const facts = deliveryFacts(delivery);
	return (
		<li className="flex items-center gap-3 border-t border-sidebar-border py-2.5 first:border-t-0">
			<Badge tone={deliveryTone(delivery.outcome)}>{deliveryLabel(delivery.outcome)}</Badge>
			<div className="flex min-w-0 flex-1 flex-col gap-0.5">
				<span className="text-[12.5px] text-foreground">
					{formatWhen(delivery.received, now)}
					<span className="text-muted-foreground">, {facts.join(", ")}</span>
				</span>
				{delivery.missing_fields.length > 0 ? (
					<span className="text-xs text-muted-foreground">
						{t("Missing {{value2}}", { value2: delivery.missing_fields.join(", ") })}
					</span>
				) : null}
				{delivery.refused_fields?.length ? (
					<span className="text-xs text-muted-foreground">
						{t("Left out free text {{value2}}", {
							value2: delivery.refused_fields.join(", "),
						})}
					</span>
				) : null}
				{delivery.error ? (
					<span className="text-xs text-status-error-text">{delivery.error}</span>
				) : null}
			</div>
		</li>
	);
};

const Runs = ({ task, onLeave }: { task: Task; onLeave: () => void }) => {
	const { t } = useI18n();

	const runs = useRuns(task.id);
	const listed = runs.data?.status === "ok" ? runs.data.data : null;
	if (!listed)
		return (
			<Empty>
				{runs.data || runs.error ? t("Couldn't read the runs") : t("Reading the runs")}
			</Empty>
		);
	if (listed.length === 0) return <Empty>{t("No runs yet")}</Empty>;
	return (
		<ul aria-label={t("Runs")}>
			{listed.map((run) => (
				<RunRow key={`${run.started}:${run.cause}`} run={run} onLeave={onLeave} />
			))}
		</ul>
	);
};

const Deliveries = ({ task }: { task: Task }) => {
	const { t } = useI18n();

	const deliveries = useDeliveries(task.id);
	const listed = deliveries.data?.status === "ok" ? deliveries.data.data : null;
	if (!listed)
		return (
			<Empty>
				{deliveries.data || deliveries.error
					? t("Couldn't read the requests")
					: t("Reading the requests")}
			</Empty>
		);
	if (listed.length === 0) return <Empty>{t("No requests yet")}</Empty>;
	return (
		<ul aria-label={t("Requests")}>
			{listed.map((d) => (
				<DeliveryRow key={d.id} delivery={d} />
			))}
		</ul>
	);
};

const TAB =
	"h-[28px] rounded-[6px] px-2.5 text-[12.5px] text-secondary-foreground hover:bg-secondary data-active:bg-selected data-active:text-foreground";

/** The Deliveries and runs dialog, open while `task` isn't null. */
export const TaskActivity = ({ task, onClose }: { task: Task | null; onClose: () => void }) => {
	const { t } = useI18n();

	const refresh = useRefreshActivity();
	const requests = task ? takesRequests(task.trigger.type) : false;
	return (
		<Dialog.Root
			open={task !== null}
			onOpenChange={(next) => {
				if (!next) onClose();
			}}
		>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup className="fixed top-1/2 left-1/2 z-[121] flex max-h-[calc(100vh-64px)] w-[min(600px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 overflow-hidden rounded-xl border border-input bg-card p-5 shadow-popup outline-none">
					{task ? (
						<Tabs.Root
							defaultValue={requests ? "requests" : "runs"}
							className="flex min-h-0 flex-col gap-3"
						>
							<div className="flex items-center gap-3">
								<Dialog.Title className="min-w-0 flex-1 truncate text-[15px] font-[650] text-foreground-strong">
									{task.name}
								</Dialog.Title>
								<button
									type="button"
									aria-label={t("Refresh")}
									className={SECONDARY}
									onClick={() => void refresh(task.id)}
								>
									<RefreshCw aria-hidden size={13} />
								</button>
								<Dialog.Close className={SECONDARY}>{t("Close")}</Dialog.Close>
							</div>
							<Tabs.List className="flex gap-1">
								<Tabs.Tab value="requests" className={TAB}>
									{t("Deliveries")}
								</Tabs.Tab>
								<Tabs.Tab value="runs" className={TAB}>
									{t("Runs")}
								</Tabs.Tab>
							</Tabs.List>
							<div className="min-h-0 overflow-y-auto">
								<Tabs.Panel value="requests">
									<Deliveries task={task} />
								</Tabs.Panel>
								<Tabs.Panel value="runs">
									<Runs task={task} onLeave={onClose} />
								</Tabs.Panel>
							</div>
						</Tabs.Root>
					) : null}
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
