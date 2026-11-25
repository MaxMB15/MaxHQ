// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The plan usage meter in the header and the popover it opens. The meter
// shows each vendor's 5-hour window, with the week as a lighter fill under
// it. The popover shows every account's own windows with their reset times,
// each model's limits under them, and the agents whose context windows are
// fullest. A window at 99% or more
// reads as used up, and a used-up week leaves nothing in the 5-hour window.
// Cursor shows its month meters once a Mac turns Cursor usage on, and a
// link to its dashboard otherwise. A Codex account says how many resets it
// has banked, and Use reset spends one once you confirm. Below the accounts,
// a cloud model server with credits, such as OpenRouter, shows what's left.

import { useI18n } from "@maxhq/shared/i18n";

import { useMemo, useState } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { type Services, useServices, useServicesIfAny, useView } from "@/app/services";
import { requestConfirm } from "@/components/confirm";
import { Popover, PopoverPopup, PopoverTitle, PopoverTrigger } from "@/components/ui/popover";
import type { Account, Agent, Timestamp } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { type CreditsView, serverCreditRows } from "@maxhq/shared/local-models/credits";
import { formatAgo, formatContext, formatPercent } from "@maxhq/shared/status/format";
import {
	canUseReset,
	resetCredits,
	resetCreditsLine,
	type ResetLine,
	resetQuestion,
	resetRunning,
	type ResetSpends,
	resetSpends,
} from "@maxhq/shared/status/reset-credits";
import {
	accountName,
	accountWindows,
	atLimit,
	blockingWindow,
	CURSOR_USAGE_URL,
	cycleWindows,
	type MeterRow,
	meterRows,
	modelRows,
	type ModelUsage,
	modelUsage,
	windowShort,
} from "@maxhq/shared/status/usage";
import { type Subscription, subscriptions } from "@maxhq/shared/status/subscriptions";
import {
	accountStatus,
	meterRowLabel,
	planNote,
	resetText,
	resetTitle,
	weekNote,
} from "@maxhq/shared/status/usage-text";
import type { ViewState } from "@maxhq/shared/view/replica";
import {
	accounts as accountList,
	fullestContexts,
	newestUsageCheck,
} from "@maxhq/shared/view/select";

/** A context window this full turns its bar yellow. */
const CONTEXT_FULL = 90;

/** A percentage as a bar width, kept within the track. */
const width = (percent: number | null) => `${Math.max(0, Math.min(100, percent ?? 0))}%`;

/**
 * A usage bar, yellow once full. An `under` figure, such as the week under
 * the 5 hours, shows as a lighter fill beneath the main one, so it reads only
 * past the main fill's end.
 */
export const Bar = ({
	percent,
	full,
	className,
	fill = "bg-secondary-foreground",
	under,
}: {
	percent: number | null;
	full: boolean;
	className: string;
	fill?: string;
	under?: number | null;
}) => {
	useI18n();

	const color = full ? "bg-status-busy" : fill;
	return (
		<span aria-hidden className={cn("relative block overflow-hidden bg-track", className)}>
			{under != null ? (
				<span
					data-layer="under"
					className={cn("absolute inset-y-0 left-0 opacity-35", color)}
					style={{ width: width(under) }}
				/>
			) : null}
			<span
				data-layer="main"
				className={cn("relative block h-full", color)}
				style={{ width: width(percent) }}
			/>
		</span>
	);
};

const Meter = ({ rows }: { rows: readonly MeterRow[] }) => {
	const { t } = useI18n();
	return (
		<>
			{rows.map((row) => (
				<span
					key={row.vendor}
					title={meterRowLabel(row)}
					className="flex items-center gap-1.5 text-[11px] text-secondary-foreground"
				>
					<span className="w-11 text-left">{row.label}</span>
					<Bar
						percent={row.percent}
						under={row.weekPercent}
						full={row.usedUp}
						className="h-1 w-16 rounded-[2px]"
					/>
					<span
						className={cn(
							"w-7 text-right tabular-nums",
							row.usedUp && "text-status-busy"
						)}
					>
						{row.percent === null ? "" : formatPercent(row.percent)}
					</span>
				</span>
			))}
			{rows.length === 0 ? (
				<span className="text-[11px] text-muted-foreground">{t("No accounts yet")}</span>
			) : null}
		</>
	);
};

/** The columns of a window row: name, bar, percent and reset, the same in every section. */
const ROW_GRID =
	"grid grid-cols-[84px_minmax(0,1fr)_34px_132px] items-center gap-x-2 gap-y-[7px] text-[11.5px] text-secondary-foreground";

/** The windows of the account's per-model limits, folded past four rows. */
const ModelSection = ({ models, now }: { models: readonly ModelUsage[]; now: Timestamp }) => {
	const { t } = useI18n();

	const [all, setAll] = useState(false);
	const { rows, folded } = modelRows(models, all);
	if (rows.length === 0) return null;
	return (
		<div className="flex flex-col gap-[7px]">
			<span className="text-[11px] text-muted-foreground">{t("By model")}</span>
			<div className={ROW_GRID}>
				{rows.map(({ scope, label, window: w }) => (
					<div key={`${scope}:${w.minutes}`} className="contents">
						<span className="flex min-w-0 items-baseline gap-1">
							<span className="truncate" title={label}>
								{label}
							</span>
							<span className="shrink-0 text-[10.5px] text-muted-foreground">
								{windowShort(w.minutes)}
							</span>
						</span>
						<Bar
							percent={w.percent}
							full={w.usedUp}
							className="h-[3px] rounded-[2px]"
						/>
						<span
							className={cn(
								"text-right tabular-nums",
								w.usedUp && "text-status-busy"
							)}
						>
							{w.percent === null ? "" : formatPercent(w.percent)}
						</span>
						<span
							title={resetTitle(w)}
							className={cn(
								"truncate",
								w.usedUp ? "text-status-busy-text" : "text-muted-foreground"
							)}
						>
							{resetText(w, now)}
						</span>
					</div>
				))}
			</div>
			{folded > 0 || all ? (
				<button
					type="button"
					aria-expanded={all}
					onClick={() => setAll(!all)}
					className="self-start text-[11px] text-primary hover:underline"
				>
					{all ? t("Show less") : t("Show all")}
				</button>
			) : null}
		</div>
	);
};

/** Each window's reset spends, kept with its services so a line outlives the popover. */
const spends = new WeakMap<Services, ReturnType<typeof resetSpends>>();

const spendsOf = (services: Services) => {
	let found = spends.get(services);
	if (!found) {
		found = resetSpends(services.source);
		spends.set(services, found);
	}
	return found;
};

/** No spends, for a section shown without services. */
const NO_SPENDS = createStore<ResetSpends>(() => ({ spends: new Map() }));

/** The color of the line a spend leaves. */
const TONE: Record<ResetLine["tone"], string> = {
	done: "text-status-ready-text",
	quiet: "text-muted-foreground",
	failed: "text-status-error-text",
};

/**
 * A Codex account's banked resets, with Use reset where its device can
 * spend one. It asks first, since a reset can't be undone.
 */
const ResetCredits = ({ account, view }: { account: Account; view: ViewState }) => {
	const { t } = useI18n();

	const services = useServicesIfAny();
	const spender = services ? spendsOf(services) : null;
	const spend = useStore(spender?.store ?? NO_SPENDS, (s) => s.spends.get(account.id));
	const count = resetCredits(account);
	const banked = resetCreditsLine(account);
	const usable = spender !== null && canUseReset(view, account);
	const running = spend?.state === "running";
	if (!banked && !spend) return null;

	const use = async () => {
		if (!services || !spender || spender.running(account.id) || count === null) return;
		if (!(await requestConfirm(services, resetQuestion(count)))) return;
		await spender.spend(account.id);
	};
	return (
		<div className="flex flex-col gap-1">
			{banked || usable || running ? (
				<div className="flex min-h-[22px] items-center justify-between gap-2">
					<span className="text-[11px] text-muted-foreground">{banked}</span>
					{usable || running ? (
						<button
							type="button"
							disabled={running}
							aria-busy={running}
							onClick={() => void use()}
							className="h-[22px] shrink-0 rounded-[5px] border border-input bg-secondary px-2 text-[11px] whitespace-nowrap text-foreground enabled:hover:bg-accent disabled:opacity-60"
						>
							{running ? resetRunning() : t("Use reset")}
						</button>
					) : null}
				</div>
			) : null}
			{spend?.state === "done" ? (
				<p role="status" className={cn("text-[11px]", TONE[spend.line.tone])}>
					{spend.line.text}
				</p>
			) : null}
		</div>
	);
};

export const AccountSection = ({
	plan,
	view,
	now,
	first,
}: {
	plan: Subscription;
	view: ViewState;
	now: Timestamp;
	first: boolean;
}) => {
	const { t } = useI18n();

	const { account } = plan;
	const windows = [...accountWindows(account), ...cycleWindows(account)];
	const limited = atLimit(account);
	const blocking = blockingWindow(account);
	const failureNote = planNote(view, plan, now);
	const note = weekNote(windows, now);
	return (
		<section
			aria-label={accountName(account)}
			className={cn(
				"flex flex-col gap-2 border-b border-track px-3.5 pb-3",
				first ? "pt-1.5" : "pt-2.5",
				limited && "bg-status-busy/5"
			)}
		>
			<div className="flex items-baseline justify-between gap-2">
				<span className="text-[12.5px] font-semibold text-foreground">
					{accountName(account)}
				</span>
				<span
					className={cn(
						"text-[11.5px]",
						limited ? "text-status-busy-text" : "text-muted-foreground"
					)}
				>
					{accountStatus(view, plan)}
				</span>
			</div>
			{windows.length > 0 ? (
				<div className={ROW_GRID}>
					{windows.map((w) => (
						<div key={w.label} className="contents">
							<span>{w.label}</span>
							<Bar
								percent={w.percent}
								full={w.usedUp}
								className="h-[5px] rounded-[3px]"
							/>
							<span
								className={cn(
									"text-right tabular-nums",
									w.usedUp && "text-status-busy"
								)}
							>
								{w.percent === null ? "" : formatPercent(w.percent)}
							</span>
							<span
								title={resetTitle(w)}
								className={cn(
									"truncate",
									blocking?.label === w.label
										? "text-status-busy-text"
										: "text-muted-foreground"
								)}
							>
								{resetText(w, now)}
							</span>
						</div>
					))}
				</div>
			) : null}
			{note ? <p className="text-[11px] text-status-busy-text">{note}</p> : null}
			<ModelSection models={modelUsage(account)} now={now} />
			<ResetCredits account={account} view={view} />
			{failureNote ? (
				<p className="text-[11px] text-muted-foreground">{failureNote}</p>
			) : null}
			{account.vendor === "cursor" && !account.usage ? (
				<a
					href={CURSOR_USAGE_URL}
					target="_blank"
					rel="noreferrer"
					className="self-start text-[11px] text-primary hover:underline"
				>
					{t("Open the Cursor dashboard")}
				</a>
			) : null}
		</section>
	);
};

/**
 * A cloud model server's credits, below the accounts, named after the
 * server. After a failed read the last figures show dimmed, with why in the
 * tooltip.
 */
const CreditsSection = ({ name, credits }: { name: string; credits: CreditsView }) => {
	useI18n();
	return (
		<section
			aria-label={name}
			title={credits.title}
			data-stale={credits.stale || undefined}
			className={cn(
				"flex flex-col gap-2 border-b border-track px-3.5 pt-2.5 pb-3",
				credits.stale && "opacity-55"
			)}
		>
			<div className="flex items-baseline justify-between gap-2">
				<span className="truncate text-[12.5px] font-semibold text-foreground">{name}</span>
				<span className="shrink-0 text-[11.5px] text-muted-foreground">
					{credits.status}
				</span>
			</div>
			<div className={ROW_GRID}>
				{credits.bars.map(({ window: w, text }) => (
					<div key={w.label} className="contents">
						<span>{w.label}</span>
						{w.percent === null ? (
							<span />
						) : (
							<Bar
								percent={w.percent}
								full={w.usedUp}
								className="h-[5px] rounded-[3px]"
							/>
						)}
						<span
							className={cn(
								"text-right tabular-nums",
								w.usedUp && "text-status-busy"
							)}
						>
							{w.percent === null ? "" : formatPercent(w.percent)}
						</span>
						<span
							className={cn(
								"truncate",
								w.usedUp ? "text-status-busy-text" : "text-muted-foreground"
							)}
						>
							{text}
						</span>
					</div>
				))}
			</div>
		</section>
	);
};

const FullestContexts = ({ agents }: { agents: readonly Agent[] }) => {
	const { t } = useI18n();

	if (agents.length === 0) return null;
	return (
		<section
			aria-label={t("Fullest context windows")}
			className="flex flex-col gap-[7px] px-3.5 pt-2.5 pb-3"
		>
			<span className="text-xs font-semibold text-terminal">
				{t("Fullest context windows")}
			</span>
			<div className="grid grid-cols-[minmax(0,1fr)_60px_82px] items-center gap-x-2 gap-y-1.5 text-[11.5px]">
				{agents.map((agent) => {
					const ctx = agent.context!;
					const percent = (ctx.used_tokens / ctx.window_tokens) * 100;
					const full = percent >= CONTEXT_FULL;
					return (
						<div key={agent.id} className="contents">
							<span className="truncate text-secondary-foreground">
								{agent.title}
							</span>
							<Bar
								percent={percent}
								full={full}
								fill="bg-primary"
								className="h-1 rounded-[2px]"
							/>
							<span
								className={cn(
									"text-right tabular-nums",
									full ? "text-status-busy-text" : "text-muted-foreground"
								)}
							>
								{formatContext(ctx.used_tokens, ctx.window_tokens)}
							</span>
						</div>
					);
				})}
			</div>
		</section>
	);
};

const RefreshIcon = () => {
	useI18n();
	return (
		<svg
			aria-hidden
			width={12}
			height={12}
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			strokeWidth={2}
			strokeLinecap="round"
			strokeLinejoin="round"
		>
			<path d="M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5" />
		</svg>
	);
};

const UsageDetails = ({ view }: { view: ViewState }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const now = useNow(1000);
	const list = useMemo(() => subscriptions(accountList(view)), [view]);
	const checked = newestUsageCheck(view);
	return (
		<>
			<div className="flex items-center gap-2 px-3.5 pt-3 pb-2">
				<PopoverTitle className="text-[13.5px] leading-tight font-[650] text-foreground">
					{t("Plan usage")}
				</PopoverTitle>
				<span className="ml-auto text-[11px] text-muted-foreground">
					{checked !== undefined
						? t("Updated {{value1}}", { value1: formatAgo(checked, now) })
						: t("Not checked yet")}
				</span>
				<button
					type="button"
					aria-label={t("Check usage now")}
					title={t("Check usage now")}
					onClick={() => void source.refreshUsage(null)}
					className="grid size-[22px] place-items-center rounded-[5px] text-muted-foreground hover:bg-accent hover:text-foreground"
				>
					<RefreshIcon />
				</button>
			</div>
			{list.map((plan, i) => (
				<AccountSection
					key={plan.account.id}
					plan={plan}
					view={view}
					now={now}
					first={i === 0}
				/>
			))}
			{list.length === 0 ? (
				<p className="border-b border-track px-3.5 pb-3 text-[11.5px] text-muted-foreground">
					{t("No Claude, Codex or Cursor login found on your devices yet.")}
				</p>
			) : null}
			{serverCreditRows(view, now).map(({ server, view: credits }) => (
				<CreditsSection key={server.id} name={server.label} credits={credits} />
			))}
			<FullestContexts agents={fullestContexts(view, 3)} />
		</>
	);
};

/** The header meter, which opens the usage popover. */
export const UsageMeter = () => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const rows = useMemo(() => meterRows(accountList(view)), [view]);
	return (
		<Popover>
			<PopoverTrigger
				aria-label={t("Plan usage. {{value1}}", {
					value1: rows.map(meterRowLabel).join(", "),
				})}
				className="flex flex-col gap-1 rounded-md px-1.5 py-1 hover:bg-secondary data-popup-open:bg-secondary data-popup-open:shadow-[inset_0_0_0_1px_var(--input)]"
			>
				<Meter rows={rows} />
			</PopoverTrigger>
			<PopoverPopup
				side="bottom"
				align="end"
				sideOffset={6}
				className="w-[376px] rounded-xl border-input shadow-large"
			>
				<UsageDetails view={view} />
			</PopoverPopup>
		</Popover>
	);
};
