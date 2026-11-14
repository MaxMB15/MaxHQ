// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Ways from prompt to prompt in a conversation. A rail in the list's right
// gutter has a tick for each prompt you sent, where it stands, and small
// caps at its ends that step to the one before or after. The Jump to latest
// pill takes the same steps, with the place of the prompt at the top, and
// so does Option+Up/Down. A step back past the first prompt loaded loads
// older pages until it finds the one before.

import type { Virtualizer } from "@tanstack/react-virtual";
import { ChevronDown, ChevronUp } from "lucide-react";
import { type FocusEvent, type PointerEvent, useEffect, useMemo, useRef, useState } from "react";

import { placeLabel } from "@maxhq/shared/agent-view/format";
import {
	itemLine,
	type PromptMark,
	type PromptPlace,
	type PromptRow,
	type PromptStep,
	placeKey,
	promptBefore,
	promptPlace,
	promptPreview,
	promptRows,
	promptTotal,
	stepPreview,
	stepPrompt,
} from "@maxhq/shared/agent-view/prompt-jump";
import type { ItemId } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { loadUntil } from "@maxhq/shared/timeline/seek";
import type { TimelineStore } from "@maxhq/shared/timeline/store";

import { cn } from "@/lib/utils";

import { FLASH_ATTRIBUTE, FLASH_MS, type JumpTarget, seekItem } from "./jump";

type Delta = -1 | 1;

/** The prompts in a list and the steps between them. */
export type PromptSteps = {
	readonly prompts: readonly PromptRow[];
	/** How many prompts there are, or null while older pages may hold more. */
	readonly total: number | null;
	/** Whether there's a prompt to step to, loaded or in the older pages. */
	readonly shown: boolean;
	readonly place: PromptPlace;
	/** Where each prompt's row starts, as a share of the whole list. */
	readonly ticks: readonly number[];
	/** Takes a step one prompt back (-1) or on (1). */
	readonly step: (delta: Delta) => void;
	/** Goes to a prompt. */
	readonly go: (index: number) => void;
	/** What a prompt previews. */
	readonly previewAt: (index: number) => string;
	/** What a step previews, or null when there's nothing that way. */
	readonly previewOf: (delta: Delta) => string | null;
	/** Reads the place again, after a scroll. */
	readonly refresh: () => void;
};

/**
 * The prompts in a list and the steps between them. Its rows sit one place
 * after their index in the virtualizer, past the row above them.
 */
export const usePromptSteps = ({
	store,
	rows,
	virtualizer,
	target,
	hasOlder,
	atEnd,
	hold,
	toLatest,
}: {
	store: TimelineStore;
	rows: readonly ItemId[];
	virtualizer: Virtualizer<HTMLDivElement, Element>;
	target: JumpTarget | null;
	hasOlder: boolean;
	/** Whether the list shows its end. */
	atEnd: () => boolean;
	/** Stops the list following new output. */
	hold: () => void;
	toLatest: () => void;
}): PromptSteps => {
	// The store drops a row only with its item and an item's kind never
	// changes, so the prompts change only with the rows. Their words can,
	// so the previews read them when they show.
	const prompts = useMemo(() => promptRows(rows, store.getState().items), [rows, store]);
	const total = promptTotal(prompts.length, hasOlder);
	const marksOf = (): PromptMark[] =>
		prompts.map((prompt) => ({
			top: virtualizer.measurementsCache[prompt.row + 1]?.start ?? 0,
		}));
	const where = () => ({ top: virtualizer.scrollOffset ?? 0, older: hasOlder, atEnd: atEnd() });
	const placeOf = () => promptPlace(marksOf(), where());
	// Scrolling only draws again when the place among the prompts changes.
	const [, setKey] = useState("");
	const place = placeOf();
	const size = virtualizer.getTotalSize();
	const ticks = marksOf().map((mark) => (size > 0 ? Math.min(mark.top / size, 1) : 0));

	const seeking = useRef<(() => void) | null>(null);
	useEffect(() => () => seeking.current?.(), []);

	const done = () => {
		seeking.current = null;
	};
	// Puts the prompt at the top and lights it.
	const seekPrompt = (target: JumpTarget, id: ItemId) => {
		seeking.current = seekItem(
			target,
			id,
			(row) => {
				done();
				row.setAttribute(FLASH_ATTRIBUTE, "");
				setTimeout(() => row.removeAttribute(FLASH_ATTRIBUTE), FLASH_MS);
			},
			done,
			{ align: "start" }
		);
	};
	const go = (index: number) => {
		const prompt = prompts[index];
		if (!prompt || !target) return;
		seeking.current?.();
		hold();
		seekPrompt(target, prompt.id);
	};
	// Goes up to where older pages load, and on to the prompt before the
	// first row here once a page brings one. Without one, it stays at the top.
	const goOlder = () => {
		if (!target) return;
		seeking.current?.();
		hold();
		virtualizer.scrollToOffset(0);
		const state = store.getState();
		const before = state.items.get(state.order[0] ?? "")?.ordinal ?? Number.POSITIVE_INFINITY;
		seeking.current = loadUntil(
			target.timeline,
			(state) => promptBefore(state, before),
			(id) => seekPrompt(target, id),
			done
		);
	};
	const goTo = (step: PromptStep) => {
		switch (step.kind) {
			case "prompt":
				return go(step.index);
			case "latest":
				seeking.current?.();
				return toLatest();
			case "older":
				return goOlder();
		}
	};
	const lineOf = (index: number) =>
		itemLine(store.getState().items.get(prompts[index]?.id ?? ""));
	return {
		prompts,
		total,
		shown: prompts.length >= 2 || hasOlder,
		place,
		ticks,
		// Read afresh, since rows measure and the list scrolls between draws.
		step: (delta) => {
			const step = stepPrompt(marksOf(), where(), delta);
			if (step) goTo(step);
		},
		go,
		previewAt: (index) => promptPreview(index, total, lineOf(index)),
		previewOf: (delta) => stepPreview(delta < 0 ? place.earlier : place.later, total, lineOf),
		refresh: () => setKey(placeKey(placeOf())),
	};
};

type Hint = { readonly text: string; readonly y: number };

// Where an element's middle is, from the top of the rail around it.
const middleOf = (element: Element): number => {
	const rail = element.closest("nav");
	const box = element.getBoundingClientRect();
	return box.top + box.height / 2 - (rail?.getBoundingClientRect().top ?? 0);
};

const stepLabel = (delta: Delta) =>
	delta < 0 ? translate("Earlier prompt") : translate("Later prompt");

/**
 * The rail, in the list's right gutter, clear of the text and of the
 * scroll bar. The list shows it only when there's a prompt to step to.
 */
export const PromptRail = ({ steps }: { steps: PromptSteps }) => {
	useI18n();
	const [hint, setHint] = useState<Hint | null>(null);
	const show = (text: string | null, event: PointerEvent | FocusEvent) =>
		setHint(text === null ? null : { text, y: middleOf(event.currentTarget) });
	const hide = () => setHint(null);
	const { prompts, place, ticks } = steps;
	const cap = (delta: Delta) => {
		const preview = steps.previewOf(delta);
		const Icon = delta < 0 ? ChevronUp : ChevronDown;
		const hintText = preview === null ? null : `${delta < 0 ? "↑" : "↓"} ${preview}`;
		return (
			<button
				type="button"
				aria-label={stepLabel(delta)}
				aria-disabled={preview === null}
				onClick={() => {
					steps.step(delta);
					hide();
				}}
				onPointerEnter={(event) => show(hintText, event)}
				onFocus={(event) => show(hintText, event)}
				onPointerLeave={hide}
				onBlur={hide}
				className="grid h-3.5 w-3 shrink-0 cursor-pointer place-items-center rounded-sm text-muted-foreground/50 hover:text-primary focus-visible:text-primary aria-disabled:cursor-default aria-disabled:opacity-40 aria-disabled:hover:text-muted-foreground/50"
			>
				<Icon aria-hidden size={11} strokeWidth={2.2} />
			</button>
		);
	};
	return (
		<nav
			aria-label={translate("Your prompts")}
			className="absolute top-2 right-4 bottom-2 z-10 flex w-3 flex-col items-center"
		>
			{cap(-1)}
			<div className="relative my-1.5 w-full flex-1">
				{prompts.map((prompt, index) => {
					const preview = steps.previewAt(index);
					const current = index === place.at;
					return (
						<button
							key={prompt.id}
							type="button"
							aria-label={preview}
							aria-current={current ? "location" : undefined}
							onClick={() => {
								steps.go(index);
								hide();
							}}
							onPointerEnter={(event) => show(preview, event)}
							onFocus={(event) => show(preview, event)}
							onPointerLeave={hide}
							onBlur={hide}
							style={{ top: `${(ticks[index] ?? 0) * 100}%` }}
							className="group absolute left-0 grid h-2 w-3 -translate-y-1/2 cursor-pointer place-items-center"
						>
							<span
								className={cn(
									"h-[2px] rounded-full",
									current
										? "w-3 bg-primary"
										: "w-2 bg-muted-foreground/35 group-hover:bg-foreground group-focus-visible:bg-foreground"
								)}
							/>
						</button>
					);
				})}
			</div>
			{cap(1)}
			{hint ? (
				<div
					role="tooltip"
					style={{ top: hint.y }}
					className="pointer-events-none absolute right-5 max-w-[280px] -translate-y-1/2 truncate rounded-md border border-input bg-popover px-2 py-1 text-[11.5px] whitespace-nowrap text-popover-foreground shadow-md"
				>
					{hint.text}
				</div>
			) : null}
		</nav>
	);
};

/** The arrows and the place at the left of the Jump to latest pill. */
export const PillSteps = ({ steps }: { steps: PromptSteps }) => {
	useI18n();
	const button = (delta: Delta) => {
		const preview = steps.previewOf(delta);
		const Icon = delta < 0 ? ChevronUp : ChevronDown;
		return (
			<button
				type="button"
				aria-label={stepLabel(delta)}
				title={preview ?? undefined}
				disabled={preview === null}
				onClick={() => steps.step(delta)}
				className="grid size-6 cursor-pointer place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground disabled:cursor-default disabled:opacity-40 disabled:hover:bg-transparent"
			>
				<Icon aria-hidden size={13} strokeWidth={1.9} />
			</button>
		);
	};
	return (
		<>
			{button(-1)}
			{button(1)}
			{steps.total === null ? null : (
				<span className="px-1 text-[11.5px] text-muted-foreground tabular-nums">
					{placeLabel(steps.place.at, steps.total)}
				</span>
			)}
			<span aria-hidden className="mx-0.5 h-3.5 w-px bg-border" />
		</>
	);
};
