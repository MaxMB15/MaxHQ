// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A wait for a usage limit to reset. While it lasts, an agent MaxHQ holds
// can move to another account with room left, or stop carrying on by itself
// once the limit resets.

import { Clock } from "lucide-react";
import { useMemo, useState } from "react";
import { useStore } from "zustand";

import { openAgent } from "@/app/open-agent";
import { useServices, useView } from "@/app/services";
import { waitingMessages } from "@/composer";
import type { ActionResult } from "@maxhq/shared/data/source";
import type { Account, AgentId, TimelineItem } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { startFailure } from "@maxhq/shared/new-agent/spec";
import { agreedMode } from "@/settings/auto-consent";
import { sendFirstSettingsFor } from "@/settings/project";
import { startingMode } from "@maxhq/shared/settings/permission";
import {
	limitWaitBody,
	limitWaitEnded,
	limitWaitTitle,
	stopResume,
} from "@maxhq/shared/agent-view/limit-wait";
import { continuesAfterLimit } from "@maxhq/shared/settings/limits";
import { accountName } from "@maxhq/shared/status/usage";

import { useRow } from "@maxhq/shared/agent-view/context";
import { isOpen, tookOf } from "@maxhq/shared/agent-view/format";
import { briefOf, moveSpec, moveTarget, roomLeft } from "../move";
import { Divider } from "./notices";
import type { ItemOf } from "./types";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const NONE_HIDDEN: ReadonlySet<string> = new Set();

const BUTTON =
	"inline-flex h-[26px] shrink-0 cursor-pointer items-center rounded-md px-2.5 text-[12px] whitespace-nowrap disabled:cursor-default disabled:opacity-50";

const Actions = ({
	agent,
	account,
	target,
	waiting,
	resumes,
	carriesOn,
}: {
	agent: AgentId;
	account: Account | undefined;
	target: Account | undefined;
	waiting: readonly TimelineItem[];
	resumes: boolean;
	carriesOn: boolean;
}) => {
	useI18n();
	const services = useServices();
	const { source } = services;
	const { timeline } = useRow();
	const [busy, setBusy] = useState(false);
	const [failure, setFailure] = useState<string | null>(null);

	const move = async () => {
		const record = services.view.getState().view.tables.agent.get(agent);
		if (!record || !target) return;
		setBusy(true);
		setFailure(null);
		const state = timeline.getState();
		const items = state.order.flatMap((id) => state.items.get(id) ?? []);
		const moved = moveSpec(record, target, briefOf(record, account, items, waiting));
		let result: ActionResult;
		try {
			// An Auto you didn't pick, as the device's default, asks first.
			const view = services.view.getState().view;
			const mode =
				moved.permission ?? startingMode(view, target.device, target.vendor, target.acp);
			const agreed = await agreedMode(services, mode, target.device);
			if (agreed.mode === null) {
				setFailure(agreed.failure);
				setBusy(false);
				return;
			}
			if (agreed.failure) console.warn(agreed.failure);
			const spec = agreed.mode === mode ? moved : { ...moved, permission: agreed.mode };
			await sendFirstSettingsFor(services, spec);
			result = await source.startAgent(spec);
		} catch (error) {
			console.warn(translate("Couldn't start the agent"), error);
			setFailure(
				translate("Couldn't start an agent on {{accountName}}. Try again.", {
					accountName: accountName(target),
				})
			);
			setBusy(false);
			return;
		}
		if (result.status !== "ok" || result.data.status !== "started") {
			setFailure(
				translate("Couldn't start an agent on {{accountName}}. {{startFailure}}.", {
					accountName: accountName(target),
					startFailure: startFailure(result),
				})
			);
			setBusy(false);
			return;
		}
		openAgent(services, result.data.agent);
		// The brief carries the waiting messages, so this agent drops them
		// and stops rather than carry on beside the new one once the limit
		// resets.
		const kept = await stopResume(source, agent, waiting, carriesOn);
		if (kept)
			setFailure(
				translate(
					"Started on {{accountName}}, but this agent may still carry on. {{kept}}",
					{ accountName: accountName(target), kept: kept }
				)
			);
		setBusy(false);
	};

	const dontResume = async () => {
		setBusy(true);
		setFailure(null);
		setFailure(await stopResume(source, agent, waiting, carriesOn));
		setBusy(false);
	};

	const room = target ? roomLeft(target) : null;
	const drop = (resumes && waiting.length > 0) || carriesOn;
	if (!target && !drop && !failure) return null;
	return (
		<div className="flex flex-col gap-1.5">
			<div className="flex flex-wrap items-center gap-1.5">
				{target ? (
					<button
						type="button"
						disabled={busy}
						onClick={() => void move()}
						className={cn(
							BUTTON,
							"border border-input bg-secondary text-foreground hover:bg-accent"
						)}
					>
						{translate("Move to ")}
						{accountName(target)}
					</button>
				) : null}
				{drop ? (
					<button
						type="button"
						disabled={busy}
						onClick={() => void dontResume()}
						className={cn(
							BUTTON,
							"text-secondary-foreground hover:bg-border hover:text-foreground"
						)}
					>
						{translate("Don't resume")}
					</button>
				) : null}
				{target && room ? (
					<span className="ml-auto text-[11.5px] text-muted-foreground tabular-nums">
						{accountName(target)} {translate(" has ")}
						{room} {translate(" left")}
					</span>
				) : null}
			</div>
			{failure ? (
				<p role="alert" className="text-[11.5px] text-status-error-text">
					{failure}
				</p>
			) : null}
		</div>
	);
};

/** A wait for a usage limit to reset. */
export const LimitWaitCard = ({ item }: { item: ItemOf<"limit_wait"> }) => {
	useI18n();
	const { agent, timeline, readOnly } = useRow();
	const view = useView((s) => s.view);
	const queued = useStore(timeline, (s) => s.queued);
	const now = useNow(1000);
	const waiting = useMemo(() => waitingMessages(queued, NONE_HIDDEN), [queued]);

	if (!isOpen(item.status)) {
		return (
			<Divider icon={<Clock size={12} strokeWidth={1.9} aria-hidden />}>
				{limitWaitEnded(tookOf(item))}
			</Divider>
		);
	}

	const account = item.account ? view.tables.account.get(item.account) : undefined;
	const record = view.tables.agent.get(agent);
	const device = record ? view.tables.workspace.get(record.workspace)?.device : undefined;
	const target =
		!readOnly && record && device
			? moveTarget(view.tables.account.values(), record, device)
			: undefined;
	// maxd stops the resume once you take back the last message it would
	// resume with, and starts it again when you queue something new. Its
	// switch for continuing after the limit carries it on too.
	const carriesOn = record !== undefined && continuesAfterLimit(view, record);
	const resumes = item.auto_resume || carriesOn;

	const title = limitWaitTitle(item.resets_at);
	const body = limitWaitBody(account, resumes, item.resets_at, now);

	return (
		<section
			aria-label={translate("Waiting for usage limit reset")}
			className="flex items-start gap-[11px] rounded-lg border border-status-busy/30 bg-status-busy/6 p-3"
		>
			<Clock
				size={15}
				strokeWidth={1.9}
				aria-hidden
				className="mt-0.5 shrink-0 text-status-busy"
			/>
			<div className="flex min-w-0 flex-1 flex-col gap-2">
				<div className="flex flex-col gap-1">
					<h3 className="text-[13px] font-semibold text-foreground-strong tabular-nums">
						{title}
					</h3>
					<p className="text-[12px] leading-normal text-secondary-foreground tabular-nums">
						{body}
					</p>
				</div>
				{readOnly ? null : (
					<Actions
						agent={agent}
						account={account}
						target={target}
						waiting={waiting}
						resumes={resumes}
						carriesOn={carriesOn}
					/>
				)}
			</div>
		</section>
	);
};
