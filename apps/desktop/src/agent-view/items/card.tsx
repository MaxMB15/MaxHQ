// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pieces the approval and question cards share: the green frame of a card
// that waits on you, its header, and the quiet card it becomes once it's
// answered.

import type { KeyboardEvent, ReactNode } from "react";

import type { Timestamp } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";
import { formatDuration, showHidden } from "@maxhq/shared/status/format";

import { Elapsed } from "../elapsed";
import { Spinner } from "../icons";
import { ALREADY_ANSWERED, type SendState } from "@maxhq/shared/agent-view/answer";
import { dismissSentBy } from "@maxhq/shared/agent-view/dismiss";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** Text with `backticks` drawn as code, and what it would hide spelled out. */
export const CodeTitle = ({ text }: { text: string }) => {
	useI18n();
	return (
		<>
			{showHidden(text)
				.split("`")
				.map((part, i) =>
					i % 2 === 1 ? (
						// Parts of one fixed string, so their place is their identity.
						<code
							key={i}
							className="rounded-[4px] border border-status-ready/30 bg-background/60 px-[5px] py-px font-mono text-[12px] font-medium text-foreground"
						>
							{part}
						</code>
					) : (
						part
					)
				)}
		</>
	);
};

// Whole minutes once it has waited a minute, so the label doesn't tick.
const waited = (ms: number): string =>
	formatDuration(ms < 60_000 ? ms : Math.floor(ms / 60_000) * 60_000);

/**
 * The frame of a card that waits on you. Going to the request focuses it,
 * so the keys it handles work without a click.
 */
export const WaitingCard = ({
	label,
	icon,
	title,
	name,
	since,
	children,
	onKeyDown,
}: {
	label: string;
	icon: ReactNode;
	title: ReactNode;
	/** What the agent goes by, as `rowName` says. */
	name: string;
	since: Timestamp | null;
	children: ReactNode;
	onKeyDown?: (event: KeyboardEvent<HTMLElement>) => void;
}) => {
	useI18n();
	return (
		<section
			aria-label={label}
			tabIndex={-1}
			data-waiting-card=""
			onKeyDown={onKeyDown}
			className="group/card flex flex-col gap-2.5 rounded-lg border border-status-ready/45 bg-status-ready/7 p-3 outline-none focus:border-status-ready focus:ring-2 focus:ring-status-ready/25"
		>
			<header className="flex items-start gap-[9px]">
				<span className="grid h-5 shrink-0 place-items-center text-status-ready">
					{icon}
				</span>
				<h3 className="min-w-0 flex-1 text-[13px] leading-5 font-semibold break-words text-foreground-strong select-text">
					{title}
				</h3>
				<span className="shrink-0 pt-px text-[11.5px] leading-5 whitespace-nowrap text-status-ready-text tabular-nums">
					{name} {translate(" is waiting")}
					{since !== null ? (
						<>
							, <Elapsed since={since} format={waited} />
						</>
					) : null}
				</span>
			</header>
			{children}
		</section>
	);
};

/** A main button, with the Enter hint. */
export const PrimaryButton = ({
	children,
	onClick,
	disabled,
	busy,
	hint = true,
	submit = false,
}: {
	children: ReactNode;
	onClick?: () => void;
	disabled?: boolean;
	busy?: boolean;
	hint?: boolean;
	/** Set to send the form it's in. */
	submit?: boolean;
}) => {
	useI18n();
	return (
		<button
			type={submit ? "submit" : "button"}
			onClick={onClick}
			disabled={disabled}
			className="inline-flex h-7 cursor-pointer items-center gap-2 rounded-md bg-primary px-2.5 text-[12px] font-semibold text-primary-foreground shadow-lit hover:bg-primary/90 disabled:cursor-default disabled:opacity-60"
		>
			{busy ? (
				<Spinner
					size={11}
					className="border-primary-foreground/30 border-t-primary-foreground"
				/>
			) : null}
			{children}
			{hint ? (
				<span aria-hidden className="text-[10.5px] font-medium opacity-70">
					⏎
				</span>
			) : null}
		</button>
	);
};

/** A plain button beside the main one. */
export const SecondaryButton = ({
	children,
	onClick,
	disabled,
	keys,
	className,
}: {
	children: ReactNode;
	onClick: () => void;
	disabled?: boolean;
	/** The key that does the same, in the form `aria-keyshortcuts` takes. */
	keys?: string;
	className?: string;
}) => {
	useI18n();
	return (
		<button
			type="button"
			onClick={onClick}
			disabled={disabled}
			aria-keyshortcuts={keys}
			className={cn(
				"inline-flex h-7 cursor-pointer items-center rounded-md border border-input bg-secondary px-2.5 text-[12px] text-foreground hover:bg-accent disabled:cursor-default disabled:opacity-60",
				className
			)}
		>
			{children}
		</button>
	);
};

/**
 * How the last answer went, under a card's buttons. `dismissed` when the
 * last one sent was Dismiss, which stops the turn rather than answer.
 */
export const SendNote = ({
	state,
	name,
	dismissed = false,
}: {
	state: SendState;
	/** What the agent goes by, as `rowName` says. */
	name: string;
	dismissed?: boolean;
}) => {
	useI18n();
	if (state.state === "failed") {
		return (
			<p role="alert" className="text-[11.5px] text-status-error-text">
				{state.message}
			</p>
		);
	}
	if (state.state === "late") {
		return (
			<p role="status" className="text-[11.5px] text-muted-foreground">
				{ALREADY_ANSWERED}.
			</p>
		);
	}
	if (state.state === "sent" && dismissed) {
		return (
			<p role="status" className="text-[11.5px] text-muted-foreground">
				{dismissSentBy(name)}
			</p>
		);
	}
	if (state.state === "sent") {
		return (
			<p role="status" className="text-[11.5px] text-status-ready-text">
				{translate("Answer sent. Waiting for ")}
				{name} {translate(" to pick it up.")}
			</p>
		);
	}
	return null;
};

/** The quiet card an approval or question becomes once it has ended. */
export const EndedCard = ({
	label,
	icon,
	title,
	meta,
	children,
}: {
	label: string;
	icon: ReactNode;
	title: ReactNode;
	meta?: ReactNode;
	children?: ReactNode;
}) => {
	useI18n();
	return (
		<section
			aria-label={label}
			className="flex flex-col gap-1.5 rounded-lg border border-border bg-subpanel px-3 py-2"
		>
			<div className="flex min-w-0 items-center gap-2 text-[12.5px]">
				<span className="grid shrink-0 place-items-center">{icon}</span>
				<span className="min-w-0 flex-1 truncate text-muted-foreground">{title}</span>
				{meta ? (
					<span className="shrink-0 text-[11.5px] whitespace-nowrap text-muted-foreground tabular-nums">
						{meta}
					</span>
				) : null}
			</div>
			{children}
		</section>
	);
};
