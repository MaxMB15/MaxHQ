// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the pill with its
// label, the slow device's time in the warning colour, the Retry or Dismiss
// link at its end, and the popover that lists this app and each device with
// its step. Before a run it offers the plan, and MaxHQ's app slot says where
// its build came from. A plan that would send a maxd no release signed asks
// first, and a device too old to check signatures says so.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useQuery } from "@tanstack/react-query";
import {} from "react";
import { useStore } from "zustand";

import { type Services, useServices, useView } from "@/app/services";
import { requestConfirm } from "@/components/confirm";
import { Popover, PopoverPopup, PopoverTitle, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import type { AppInfo } from "@maxhq/shared/fleet/wire";
import {
	describeRunDevice,
	describeUpdateCapsule,
	isActive,
	planUpdate,
	type FleetRun,
	type PlanRow,
	type UpdatePlan,
} from "@maxhq/shared/fleet/update-view";
import { useNow } from "@maxhq/shared/lib/clock";

import { dismissRun, retryFailed, startUpdate, updateRunOf } from "./update-run";

const PILL =
	"relative mx-1 mt-1.5 flex h-[30px] shrink-0 items-center rounded-full border border-track bg-secondary text-xs text-secondary-foreground";

const TRIGGER =
	"absolute inset-0 cursor-pointer rounded-full outline-hidden hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset";

const LINK =
	"relative shrink-0 cursor-pointer pr-3 pl-1.5 text-xs underline decoration-muted-foreground/60 underline-offset-3 hover:decoration-current";

const SMALL =
	"inline-flex h-6 items-center rounded-md border border-input bg-secondary px-2 text-[11.5px] text-foreground hover:bg-accent";

/** What the app slot says: its version, or that it was built here. */
export const appSlot = (info: AppInfo | null | undefined): string =>
	info?.bundled && info.bundled.length > 0
		? t("version {{version}}", { version: info.version })
		: t("built from source");

const AppLine = ({ info }: { info: AppInfo | null | undefined }) => {
	const { t } = useI18n();
	return (
		<div className="flex items-center justify-between gap-3">
			<span>{t("This app")}</span>
			<span className="selectable text-muted-foreground">{appSlot(info)}</span>
		</div>
	);
};

const planLine = (row: PlanRow): string =>
	[row.maxd ? "maxd" : null, ...row.tools].filter((part) => part !== null).join(", ");

/**
 * Starts the plan's run. One that would send a `maxd` no release signed,
 * built on this Mac, names the risk and goes only once you say Install
 * anyway, which marks those uploads unsigned so the devices take them.
 */
export const confirmAndStart = async (
	services: Services,
	plan: UpdatePlan,
	now: number
): Promise<void> => {
	if (plan.unsigned) {
		const agreed = await requestConfirm(services, {
			title: t("Install a maxd no release signed?"),
			detail: t(
				"The devices will run a maxd no release signed, built on this Mac. Anyone who could change what this Mac builds could then run their code on them."
			),
			action: t("Install anyway"),
		});
		if (!agreed) return;
	}
	startUpdate(services, plan, now, plan.unsigned);
};

const Plan = ({
	summary,
	rows,
	info,
	onStart,
}: {
	summary: string;
	rows: readonly PlanRow[];
	info: AppInfo | null | undefined;
	onStart: () => void;
}) => {
	const { t } = useI18n();
	return (
		<Popover>
			<div className={PILL}>
				<PopoverTrigger
					render={<button type="button" aria-label={summary} className={TRIGGER} />}
				/>
				<span className="pointer-events-none relative min-w-0 flex-1 truncate pl-3">
					{summary}
				</span>
				<button type="button" className={LINK} onClick={onStart}>
					{t("Update everything")}
				</button>
			</div>
			<PopoverPopup side="top" align="start" className="w-64 p-3">
				<div className="flex flex-col gap-1.5 text-xs">
					<PopoverTitle className="text-xs">{summary}</PopoverTitle>
					<AppLine info={info} />
					{rows.map((row) => (
						<div key={row.device} className="flex min-w-0 flex-col">
							<div className="flex items-center justify-between gap-3">
								<span className="truncate">{row.label}</span>
								<span className="shrink-0 truncate text-muted-foreground">
									{planLine(row)}
								</span>
							</div>
							{row.unchecked ? (
								<span
									className="text-status-connecting-text"
									title={t(
										"{{name}}'s maxd is too old to check signatures, so it installs what it's sent.",
										{ name: row.label }
									)}
								>
									{t("doesn't check signatures")}
								</span>
							) : null}
						</div>
					))}
					<div className="mt-1 flex justify-end">
						<button type="button" className={SMALL} onClick={onStart}>
							{t("Update everything")}
						</button>
					</div>
				</div>
			</PopoverPopup>
		</Popover>
	);
};

const Running = ({
	run,
	info,
	now,
	onRetry,
	onDismiss,
}: {
	run: FleetRun;
	info: AppInfo | null | undefined;
	now: number;
	onRetry: () => void;
	onDismiss: () => void;
}) => {
	const { t } = useI18n();

	const view = describeUpdateCapsule(run, now);
	const settled = !run.devices.some((d) => isActive(d.status));
	const failed = run.devices.some((d) => d.status.status === "failed");
	const act = view.action === "retry" ? onRetry : onDismiss;
	return (
		<Popover>
			<div
				role="status"
				aria-label={t("Update everything")}
				className={cn(
					PILL,
					view.tone === "slow" && "border-status-connecting/50",
					view.tone === "failed" && "border-status-error/50"
				)}
			>
				<PopoverTrigger
					render={
						<button
							type="button"
							aria-label={
								view.slow
									? `${view.label} ${view.slow.status} ${view.slow.elapsed}`
									: view.label
							}
							className={TRIGGER}
						/>
					}
				/>
				<span className="pointer-events-none relative min-w-0 flex-1 truncate pl-3">
					<span className={cn(view.tone === "failed" && "text-status-error-text")}>
						{view.label}
					</span>
					{view.slow ? (
						<span className="text-status-connecting-text tabular-nums">
							{" "}
							{view.slow.status} {view.slow.elapsed}
						</span>
					) : null}
				</span>
				{view.action !== null ? (
					<button type="button" className={LINK} onClick={act}>
						{view.action === "retry" ? t("Retry") : t("Dismiss")}
					</button>
				) : (
					<span
						aria-hidden
						className="relative mr-3 size-2.5 shrink-0 animate-busy rounded-full border-[1.5px] border-status-connecting/28 border-t-status-connecting"
					/>
				)}
			</div>
			<PopoverPopup side="top" align="start" className="w-64 p-3">
				<div className="flex flex-col gap-1 text-xs">
					<PopoverTitle className="text-xs">{t("Update everything")}</PopoverTitle>
					<AppLine info={info} />
					{run.devices.map((d) => (
						<div key={d.device} className="flex min-w-0 flex-col">
							<div className="flex items-center justify-between gap-3">
								<span title={d.label} className="selectable truncate">
									{d.label}
								</span>
								<span
									className={cn(
										"selectable shrink-0 truncate tabular-nums",
										d.status.status === "failed"
											? "text-status-error-text"
											: "text-muted-foreground"
									)}
								>
									{describeRunDevice(d, now)}
								</span>
							</div>
							{d.status.status === "failed" ? (
								<span className="selectable break-words text-muted-foreground">
									{d.status.reason}
								</span>
							) : null}
						</div>
					))}
					{settled && failed ? (
						<div className="mt-1 flex justify-end gap-1">
							<button type="button" className={SMALL} onClick={onRetry}>
								{t("Retry")}
							</button>
							<button type="button" className={SMALL} onClick={onDismiss}>
								{t("Dismiss")}
							</button>
						</div>
					) : null}
				</div>
			</PopoverPopup>
		</Popover>
	);
};

/**
 * The sidebar's Update everything capsule: the plan while something is
 * behind, then the run until every device is done or you dismiss it.
 */
export const UpdateCapsule = () => {
	useI18n();

	const services = useServices();
	const now = useNow(1000);
	const view = useView((s) => s.view);
	const run = useStore(updateRunOf(services), (s) => s.run);
	const info = useQuery({
		queryKey: ["app-info"],
		queryFn: () => services.source.appInfo(),
	}).data;
	const plan = planUpdate(view, info?.bundled ?? null);

	if (run) {
		return (
			<Running
				run={run}
				info={info}
				now={now}
				onRetry={() => retryFailed(services, now)}
				onDismiss={() => dismissRun(services)}
			/>
		);
	}
	if (plan.summary === null) return null;
	return (
		<Plan
			summary={plan.summary}
			rows={plan.rows}
			info={info}
			onStart={() => void confirmAndStart(services, plan, now)}
		/>
	);
};
