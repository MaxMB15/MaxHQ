// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The quieter items: errors, compaction, a stopped turn, the line that
// ends a turn, notices from the CLI, a move to another vendor, and anything
// MaxHQ doesn't know how to show yet.

import {
	ArrowRight,
	ArrowRightLeft,
	CircleAlert,
	CircleStop,
	Info,
	Shrink,
	TriangleAlert,
} from "lucide-react";
import type { ReactNode } from "react";

import { useServices, useView } from "@/app/services";
import { cn } from "@/lib/utils";
import { openCliSignIn } from "@/settings/cli-sign-in";
import { openSettings } from "@/settings/store";
import type { AccountId, Vendor } from "@maxhq/shared/bindings";
import { handoffTitle, isOpen } from "@maxhq/shared/agent-view/format";
import { signInAction } from "@maxhq/shared/settings/cli-sign-in";
import { formatDuration, formatTokens } from "@maxhq/shared/status/format";
import { handoffEndText } from "@maxhq/shared/local-models/handoff";

import { Spinner } from "../icons";
import { SecondaryButton } from "./card";
import type { ItemOf } from "./types";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/**
 * Sign in again, for an error a login that's signed out caused: the in-app
 * login, or the Settings page that sends that kind of login.
 */
const SignInAgain = ({ account }: { account: AccountId }) => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const action = signInAction(view, account);
	if (!action) return null;
	return (
		<div>
			<SecondaryButton
				onClick={() =>
					action.kind === "login"
						? openCliSignIn(services, action.account.id)
						: openSettings(services, {
								kind: "login",
								login: action.section,
								device: action.device,
							})
				}
			>
				{translate("Sign in again")}
			</SecondaryButton>
		</div>
	);
};

/** Something that went wrong. */
export const ErrorRow = ({ item }: { item: ItemOf<"error"> }) => {
	useI18n();
	return (
		<div className="flex items-start gap-2.5 rounded-lg border border-destructive/30 bg-destructive/6 px-3 py-2 text-[12.5px]">
			<CircleAlert
				size={14}
				strokeWidth={1.9}
				aria-hidden
				className="mt-[3px] shrink-0 text-destructive"
			/>
			<div className="flex min-w-0 flex-1 flex-col gap-2">
				<p
					data-find="plain"
					className="leading-normal break-words text-foreground select-text"
				>
					{item.message}
				</p>
				{item.sign_in ? <SignInAgain account={item.sign_in} /> : null}
			</div>
			{item.retrying ? (
				<span className="inline-flex shrink-0 items-center gap-1.5 pt-0.5 text-[11.5px] text-status-error-text">
					<Spinner size={10} className="border-destructive/25 border-t-destructive" />
					{translate("Retrying")}
				</span>
			) : null}
		</div>
	);
};

/** A line across the column with a few words in the middle. */
export const Divider = ({ icon, children }: { icon: ReactNode; children: ReactNode }) => {
	useI18n();
	return (
		<div className="flex items-center gap-3 py-0.5 text-[11.5px] text-muted-foreground">
			<span className="h-px min-w-3 flex-1 bg-border" />
			<span className="inline-flex min-w-0 items-center gap-1.5 tabular-nums">
				{icon}
				<span className="min-w-0 truncate">{children}</span>
			</span>
			<span className="h-px min-w-3 flex-1 bg-border" />
		</div>
	);
};

/** The CLI shrinking the conversation to fit. */
export const CompactionRow = ({ item }: { item: ItemOf<"compaction"> }) => {
	useI18n();
	return (
		<Divider
			icon={
				isOpen(item.status) ? (
					<Spinner size={10} />
				) : (
					<Shrink size={12} strokeWidth={1.9} aria-hidden />
				)
			}
		>
			{isOpen(item.status)
				? translate("Compacting the conversation")
				: item.before_tokens !== null && item.after_tokens !== null
					? translate("Compacted the conversation, {{tokens}} to {{tokens2}} tokens", {
							tokens: formatTokens(item.before_tokens),
							tokens2: formatTokens(item.after_tokens),
						})
					: translate("Compacted the conversation")}
		</Divider>
	);
};

/** A turn you stopped. */
export const InterruptedRow = () => {
	useI18n();
	return (
		<Divider icon={<CircleStop size={12} strokeWidth={1.9} aria-hidden />}>
			{translate("You stopped this turn")}
		</Divider>
	);
};

/** The line that ends a turn. */
export const TurnSummaryRow = ({ item }: { item: ItemOf<"turn_summary"> }) => {
	useI18n();
	const parts: string[] = [];
	if (item.duration_ms !== null)
		parts.push(
			translate("Worked for {{duration}}", { duration: formatDuration(item.duration_ms) })
		);
	if (item.context_tokens !== null)
		parts.push(
			translate("{{tokens}} tokens of context", { tokens: formatTokens(item.context_tokens) })
		);
	return (
		<Divider icon={null}>
			{parts.length > 0 ? parts.join(", ") : translate("Turn ended")}
		</Divider>
	);
};

/** A message from the CLI or maxd. */
export const NoticeRow = ({ item }: { item: ItemOf<"notice"> }) => {
	useI18n();
	const warning = item.level === "warning";
	return (
		<div
			className={cn(
				"flex items-start gap-2 px-1.5 text-[12.5px] leading-normal",
				warning ? "text-status-busy-text" : "text-muted-foreground"
			)}
		>
			{warning ? (
				<TriangleAlert
					size={14}
					strokeWidth={1.9}
					aria-hidden
					className="mt-[3px] shrink-0 text-status-busy"
				/>
			) : (
				<Info size={14} strokeWidth={1.9} aria-hidden className="mt-[3px] shrink-0" />
			)}
			<p data-find="plain" className="min-w-0 break-words select-text">
				{item.message}
			</p>
		</div>
	);
};

/** One end of a move, with the model by its label. */
const HandoffEnd = ({ vendor, model }: { vendor: Vendor; model: string | null }) => {
	useI18n();
	const text = useView((s) => handoffEndText(s.view, vendor, model));
	return <span className="font-medium text-secondary-foreground">{text}</span>;
};

/**
 * Where the agent moved to another vendor, marking the context handoff. It
 * waits until the next message carries the conversation over.
 */
export const HandoffRow = ({ item }: { item: ItemOf<"handoff"> }) => {
	useI18n();
	return (
		<Divider icon={<ArrowRightLeft size={12} strokeWidth={1.9} aria-hidden />}>
			{handoffTitle(item.status)}
			<span className="inline-flex items-center gap-1">
				<HandoffEnd vendor={item.from_vendor} model={item.from_model} />
				<ArrowRight size={11} strokeWidth={1.9} aria-label={translate("to")} />
				<HandoffEnd vendor={item.to_vendor} model={item.to_model} />
			</span>
		</Divider>
	);
};

/** Something the driver can't show yet. */
export const UnknownRow = ({ item }: { item: ItemOf<"unknown"> }) => {
	useI18n();
	return (
		<div className="flex items-center gap-2 px-1.5 text-[12.5px] text-muted-foreground">
			<Info size={14} strokeWidth={1.9} aria-hidden className="shrink-0" />
			<span>{translate("MaxHQ can't show this item yet")}</span>
			<span className="truncate font-mono text-[11px] text-subtle-foreground select-text">
				{item.native_kind}
			</span>
		</div>
	);
};
