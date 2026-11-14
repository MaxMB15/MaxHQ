// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A card that asks before MaxHQ lets something through: a title naming
// what's asked, the exact risk in a sentence or two, the details to judge
// it by, the answers, and where the answer is kept and changed.

import { ShieldAlert } from "lucide-react";
import type { ReactNode } from "react";

import { type ConsentFact, ConsentFacts } from "@/components/consent-facts";
import { PRIMARY, SECONDARY, StatusNote } from "@/settings/controls";
import type { Note } from "@/settings/result";

/** One group of details, such as one link's addresses and host keys. */
export type ConsentDetails = {
	readonly title: string;
	readonly facts: readonly ConsentFact[];
};

/** One answer button. */
export type ConsentAnswer = {
	readonly label: string;
	readonly onAnswer: () => void;
	/** Drawn as the main button. Leave it off when neither answer is safer. */
	readonly primary?: boolean;
};

export const ConsentCard = ({
	title,
	risk,
	details,
	answers,
	remember,
	note,
	busy,
	children,
}: {
	title: string;
	risk: string;
	details: readonly ConsentDetails[];
	answers: readonly ConsentAnswer[];
	/** Where the answer is kept and changed. */
	remember: string;
	note?: Note | null;
	busy?: boolean;
	/** Shown under the details, such as while they load. */
	children?: ReactNode;
}) => (
	<section
		aria-label={title}
		className="overflow-hidden rounded-[11px] border border-status-connecting/45 bg-card"
	>
		<div className="flex items-start gap-2.5 border-b border-sidebar-border px-4 py-3">
			<ShieldAlert
				aria-hidden
				size={16}
				className="mt-px shrink-0 text-status-connecting-text"
			/>
			<h2 className="text-[13.5px] font-[650] text-foreground">{title}</h2>
		</div>
		<p className="max-w-[68ch] px-4 pt-3 text-[12.5px] text-secondary-foreground">{risk}</p>
		{details.length > 0 ? (
			<div className="grid gap-3 px-4 pt-3 sm:grid-cols-2">
				{details.map((group) => (
					<div
						key={group.title}
						className="flex min-w-0 flex-col gap-1.5 rounded-[8px] border border-track bg-subpanel px-3 py-2.5"
					>
						<h3 className="truncate text-xs font-[650] text-foreground">
							{group.title}
						</h3>
						<ConsentFacts facts={group.facts} />
					</div>
				))}
			</div>
		) : null}
		{children ? <div className="px-4 pt-3">{children}</div> : null}
		<div className="mt-3 flex flex-wrap items-center gap-2.5 border-t border-sidebar-border px-4 py-3">
			<span className="min-w-0 flex-1 text-[11.5px] text-muted-foreground">{remember}</span>
			<StatusNote note={note ?? null} />
			{answers.map((answer) => (
				<button
					key={answer.label}
					type="button"
					className={answer.primary ? PRIMARY : SECONDARY}
					disabled={busy}
					onClick={answer.onAnswer}
				>
					{answer.label}
				</button>
			))}
		</div>
	</section>
);
