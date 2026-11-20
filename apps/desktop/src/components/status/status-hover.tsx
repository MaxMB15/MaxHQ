// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The hover card behind every dot, drawn as the Indicators mockup draws it:
// a title beside its dot, a sentence, the check bar while a connection
// attempt runs, label and value rows, and a small last line. The card is
// built only while it's open, with its own clock, so a closed hover costs
// nothing.

import { t, useI18n } from "@maxhq/shared/i18n";

import type { CSSProperties, ReactNode } from "react";

import { PreviewCard, PreviewCardPopup, PreviewCardTrigger } from "@/components/ui/preview-card";
import type { Timestamp } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import type { Check, CheckState, HoverCard } from "@maxhq/shared/status/card";
import { listInWords } from "@maxhq/shared/status/format";

const SEGMENT: Record<CheckState, string> = {
	passed: "bg-status-ready",
	running: "bg-status-connecting",
	failed: "bg-status-error",
	not_run: "bg-input",
	unknown: "bg-input",
};

const SEGMENT_LABEL: Record<CheckState, string> = {
	passed: "text-muted-foreground",
	running: "text-status-connecting-text",
	failed: "text-status-error-text",
	not_run: "text-muted-foreground",
	unknown: "text-muted-foreground",
};

const SUMMARY: [CheckState, string][] = [
	["passed", "passed"],
	["running", "in progress"],
	["failed", "failed"],
	["not_run", "not run"],
	["unknown", "not checked"],
];

/** What a screen reader hears for the bar, such as "Checks: Net and Peer passed, SSH in progress". */
export const checksSummary = (checks: readonly Check[]): string => {
	const parts = SUMMARY.flatMap(([state, words]) => {
		const names = checks.filter((c) => c.state === state).map((c) => c.short);
		return names.length > 0
			? [t("{{names}} {{state}}", { names: listInWords(names), state: t(words) })]
			: [];
	});
	return t("Checks: {{value1}}", { value1: parts.join(", ") });
};

/** The row of check segments with their names under them. */
export const CheckBar = ({ checks }: { checks: readonly Check[] }) => {
	useI18n();
	return (
		<div
			role="img"
			aria-label={checksSummary(checks)}
			className="grid gap-x-[3px] gap-y-1"
			style={{ gridTemplateColumns: `repeat(${checks.length}, minmax(0, 1fr))` }}
		>
			{checks.map((c) => (
				<span key={c.id} className={cn("h-[5px] rounded-[2px]", SEGMENT[c.state])} />
			))}
			{checks.map((c) => (
				<span
					key={c.id}
					aria-hidden
					className={cn("truncate text-[10px] leading-tight", SEGMENT_LABEL[c.state])}
				>
					{c.short}
				</span>
			))}
		</div>
	);
};

/** The inside of a hover card. */
export const HoverCardBody = ({ card, dot }: { card: HoverCard; dot: ReactNode }) => {
	useI18n();
	return (
		<div
			className={cn("flex flex-col", card.checks ? "gap-[9px]" : "gap-2")}
			style={{ "--dot-surface": "var(--hover-card)" } as CSSProperties}
		>
			<div className="flex items-center gap-2">
				{dot}
				<p className="text-[13px] font-semibold text-foreground-strong">{card.title}</p>
			</div>
			{card.body ? (
				<p className="selectable text-xs leading-normal text-secondary-foreground">
					{card.body}
				</p>
			) : null}
			{card.checks && card.checks.length > 0 ? <CheckBar checks={card.checks} /> : null}
			{card.facts.length > 0 ? (
				<dl className="grid grid-cols-[70px_minmax(0,1fr)] gap-x-2 gap-y-[3px] text-[11.5px]">
					{card.facts.map((f) => (
						<div key={f.label} className="contents">
							<dt className="text-muted-foreground">{f.label}</dt>
							<dd className="selectable min-w-0 text-terminal">{f.value}</dd>
						</div>
					))}
				</dl>
			) : null}
			{card.foot ? <p className="text-[11px] text-muted-foreground">{card.foot}</p> : null}
		</div>
	);
};

const LiveCard = ({ card, dot }: { card: (now: Timestamp) => HoverCard; dot: ReactNode }) => {
	useI18n();

	const now = useNow(1000);
	return <HoverCardBody card={card(now)} dot={dot} />;
};

/** A dot that opens its hover card when the pointer rests on it. */
export const StatusHover = ({
	children,
	dot,
	card,
	className,
}: {
	/** The dot the card opens from. */
	children: ReactNode;
	/** The same dot at 10 px, drawn beside the card's title. */
	dot: ReactNode;
	/** Builds the card for a moment. It runs only while the card is open. */
	card: (now: Timestamp) => HoverCard;
	className?: string;
}) => {
	useI18n();
	return (
		<PreviewCard>
			<PreviewCardTrigger
				delay={250}
				closeDelay={80}
				render={<span />}
				className={cn("inline-grid shrink-0 place-items-center p-1 -m-1", className)}
			>
				{children}
			</PreviewCardTrigger>
			<PreviewCardPopup
				side="right"
				align="start"
				sideOffset={10}
				className="w-[300px] rounded-[10px] border-hover-card-border bg-hover-card px-3.5 py-3 shadow-hover"
			>
				<LiveCard card={card} dot={dot} />
			</PreviewCardPopup>
		</PreviewCard>
	);
};
