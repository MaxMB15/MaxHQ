// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Going back to a quoted passage. A quote pill's click, in a composer or a
// sent bubble, and the Jump to button in its preview, open the quoted
// agent's timeline, scroll to the passage and highlight it for a moment.
// The timeline may not have the item yet, so a jump loads older pages
// until it does, and each list in the page says where its rows are. Find
// goes to its matches the same way.

import type { AgentId, ItemId } from "@maxhq/shared/bindings";
import type { QuoteRef } from "@maxhq/shared/pills/draft";
import { type LoadFailure, loadItem } from "@maxhq/shared/timeline/seek";
import type { Timeline } from "@maxhq/shared/timeline/store";
import { showFailure } from "@maxhq/shared/ui/inbox-notice";

import { openAgent } from "@/app/open-agent";
import type { Services } from "@/app/services";
import { noticesOf } from "@/sidebar/notices";

import { QUOTE_ROW, resolveQuoteRange } from "./selection";
import { t as translate } from "@maxhq/shared/i18n";

/** How long the passage stays lit, in milliseconds. */
export const FLASH_MS = 1600;
/** The highlight that lights the passage, which quotes.css paints. */
export const JUMP_HIGHLIGHT = "quote-jump";
/** Lights a whole row when the passage can't be lit on its own. */
export const FLASH_ATTRIBUTE = "data-quote-flash";
// Frames a jump waits for the item's row to show and hold still.
const MAX_FRAMES = 90;
// Frames between asks to scroll to the row while it isn't in the page yet.
const RETRY_FRAMES = 10;
// How long a jump can take before it gives up.
const GIVE_UP_MS = 15_000;

/** Where a row goes when the list scrolls to it. */
export type RevealAlign = "center" | "start";

/** What a mounted timeline lets a jump do. */
export type JumpTarget = {
	readonly timeline: Timeline;
	/** The element that scrolls, with the rows in it. */
	readonly scroller: HTMLElement;
	/**
	 * Scrolls the item's row toward the middle, or to the top with `align`
	 * "start". Returns false when the list has no such row.
	 */
	readonly reveal: (item: ItemId, align?: RevealAlign) => boolean;
	/** Stops the list following new output, so a scroll to a row stays. */
	readonly hold: () => void;
};

/** What a jump asks of the app around it. */
export type JumpApp = {
	/** Opens the agent's pane, or focuses it. */
	readonly open: (agent: AgentId) => void;
	/** The agent's title, or null when it's gone. */
	readonly titleOf: (agent: AgentId) => string | null;
	/** Says why the jump went nowhere. */
	readonly fail: (text: string) => void;
};

/** The jumps of one window. */
export type QuoteJumps = {
	/** Hands a jump the agent's mounted timeline. Returns the undo. */
	readonly register: (agent: AgentId, target: JumpTarget) => () => void;
	/** Goes to a quote, stopping any jump under way. */
	readonly jump: (quote: QuoteRef, app: JumpApp) => void;
};

/** Whether the webview can light ranges with the CSS Custom Highlight API. */
export const supportsHighlights = () =>
	typeof CSS !== "undefined" && "highlights" in CSS && typeof Highlight !== "undefined";

// Only the rows in view are in the page, so looking through them is cheap.
export const rowIn = (scroller: HTMLElement, item: ItemId): HTMLElement | null => {
	for (const row of scroller.querySelectorAll<HTMLElement>(`[${QUOTE_ROW}]`))
		if (row.getAttribute(QUOTE_ROW) === item) return row;
	return null;
};

const overlaps = (rect: DOMRect, box: DOMRect) => rect.bottom >= box.top && rect.top <= box.bottom;

/** Whether `rect` shows whole in `scroller`. */
export const showsIn = (scroller: HTMLElement, rect: DOMRect): boolean => {
	const box = scroller.getBoundingClientRect();
	return rect.top >= box.top && rect.bottom <= box.bottom;
};

/** Scrolls `scroller` so `rect` sits in its middle, or its top when it's taller. */
export const bringIntoView = (scroller: HTMLElement, rect: DOMRect) => {
	if (showsIn(scroller, rect)) return;
	const box = scroller.getBoundingClientRect();
	scroller.scrollTop += rect.top - box.top - Math.max(0, (box.height - rect.height) / 2);
};

/** Lights the passage, or its whole row when it's gone or the webview can't light a range. */
const flash = (row: HTMLElement, range: Range | null) => {
	if (range !== null && supportsHighlights()) {
		const highlight = new Highlight(range);
		CSS.highlights.set(JUMP_HIGHLIGHT, highlight);
		setTimeout(() => {
			if (CSS.highlights.get(JUMP_HIGHLIGHT) === highlight)
				CSS.highlights.delete(JUMP_HIGHLIGHT);
		}, FLASH_MS);
		return;
	}
	row.setAttribute(FLASH_ATTRIBUTE, "");
	setTimeout(() => row.removeAttribute(FLASH_ATTRIBUTE), FLASH_MS);
};

/** Why going to an item stopped short. */
export type SeekFailure =
	| LoadFailure
	/** The item is there, but its row never showed. */
	| "unscrolled";

/**
 * Goes to an item: waits for the timeline to open, loads older pages until
 * it has the item, then asks the list to scroll to its row and waits for
 * the row to show and hold still. With `center` false, a row that already
 * shows stays where it is. With `align` "start", the row goes to the top
 * of the list instead of its middle. Returns the call that stops it.
 */
export const seekItem = (
	target: JumpTarget,
	item: ItemId,
	shown: (row: HTMLElement) => void,
	failed: (why: SeekFailure) => void,
	{
		center = true,
		align = "center",
	}: { readonly center?: boolean; readonly align?: RevealAlign } = {}
): (() => void) => {
	const { timeline, scroller } = target;
	const stops: (() => void)[] = [];
	let done = false;
	const stop = () => {
		done = true;
		for (const undo of stops.splice(0)) undo();
	};
	const fail = (why: SeekFailure) => {
		stop();
		failed(why);
	};

	// Waits for the row to show and hold still, asking the list to scroll to it.
	const reveal = () => {
		let frames = 0;
		let lastTop: number | null = null;
		let frame = 0;
		// The list renders the item a frame or so after its store has it.
		let asked = false;
		const tick = () => {
			if (done) return;
			const row = rowIn(scroller, item);
			const rect = row?.getBoundingClientRect();
			const shows = rect !== undefined && overlaps(rect, scroller.getBoundingClientRect());
			if (row && shows && (rect.top === lastTop || frames >= MAX_FRAMES)) {
				stop();
				return shown(row);
			}
			if (frames >= MAX_FRAMES)
				return fail(timeline.store.getState().rows.includes(item) ? "unscrolled" : "gone");
			lastTop = shows ? rect.top : null;
			// The first ask also stops the list following new output.
			if ((center || !shows) && (!asked || (!shows && frames % RETRY_FRAMES === 0)))
				asked = target.reveal(item, align) || asked;
			frames++;
			frame = requestAnimationFrame(tick);
		};
		stops.push(() => cancelAnimationFrame(frame));
		tick();
	};

	stops.push(loadItem(timeline, item, reveal, fail));
	return stop;
};

type Jump = {
	readonly quote: QuoteRef;
	readonly app: JumpApp;
	readonly title: string;
	target: JumpTarget | null;
	readonly stops: (() => void)[];
};

export const createQuoteJumps = (): QuoteJumps => {
	const targets = new Map<AgentId, JumpTarget>();
	let active: Jump | null = null;

	const stop = (jump: Jump) => {
		for (const undo of jump.stops.splice(0)) undo();
		if (active === jump) active = null;
	};

	const fail = (jump: Jump, text: string) => {
		stop(jump);
		jump.app.fail(text);
	};

	const gone = (jump: Jump) =>
		translate("The quoted message is no longer in {{title}}.", { title: jump.title });

	const failure = (jump: Jump, why: SeekFailure): string => {
		switch (why) {
			case "unopened":
				return translate("Couldn't open {{title}} to find the quote.", {
					title: jump.title,
				});
			case "unloaded":
				return translate("Couldn't load earlier items in {{title}} to find the quote.", {
					title: jump.title,
				});
			case "gone":
				return gone(jump);
			case "unscrolled":
				return translate("Couldn't scroll to the quote in {{title}}.", {
					title: jump.title,
				});
		}
	};

	// Goes to the quoted item, then lights the passage in its row.
	const find = (jump: Jump, target: JumpTarget) => {
		jump.target = target;
		const { scroller } = target;
		const stopSeek = seekItem(
			target,
			jump.quote.item,
			(row) => {
				stop(jump);
				const range = resolveQuoteRange(row, jump.quote);
				bringIntoView(scroller, (range ?? row).getBoundingClientRect());
				flash(row, range);
			},
			(why) => fail(jump, failure(jump, why))
		);
		if (active === jump) jump.stops.push(stopSeek);
	};

	return {
		register: (agent, target) => {
			targets.set(agent, target);
			if (active?.quote.agent === agent && active.target === null) find(active, target);
			return () => {
				if (targets.get(agent) !== target) return;
				targets.delete(agent);
				if (active?.target === target) stop(active);
			};
		},
		jump: (quote, app) => {
			if (active !== null) stop(active);
			const title = app.titleOf(quote.agent);
			if (title === null) {
				app.fail(
					translate("{{agent_title}} is gone, so the quote can't be shown.", {
						agent_title: quote.agent_title,
					})
				);
				return;
			}
			const jump: Jump = { quote, app, title, target: null, stops: [] };
			active = jump;
			const timer = setTimeout(
				() =>
					fail(
						jump,
						translate("Couldn't find the quote in {{title}}.", { title: title })
					),
				GIVE_UP_MS
			);
			jump.stops.push(() => clearTimeout(timer));
			app.open(quote.agent);
			// Opening a pane mounts its list later, and the list registers then.
			const target = targets.get(quote.agent);
			if (target && active === jump && jump.target === null) find(jump, target);
		},
	};
};

const jumps = createQuoteJumps();
let services: Services | null = null;
let providers = 0;

/** Hands the window's jumps the agent's mounted timeline. Returns the undo. */
export const registerJumpTarget = jumps.register;

/** Lends the jumps the window's services while a timeline is open. Returns the undo. */
export const provideJumpServices = (next: Services): (() => void) => {
	services = next;
	providers++;
	return () => {
		providers--;
		if (providers === 0) services = null;
	};
};

const appOf = (services: Services): JumpApp => ({
	open: (agent) => openAgent(services, agent),
	titleOf: (agent) => services.view.getState().view.tables.agent.get(agent)?.title ?? null,
	fail: (text) => showFailure(noticesOf(services), text),
});

/**
 * Opens the quote's agent, scrolls to the passage and highlights it. When
 * the passage is gone, it goes to the quoted item, or says so if that's gone
 * too.
 */
export const jumpToQuote: (quote: QuoteRef) => void = (quote) => {
	if (services !== null) jumps.jump(quote, appOf(services));
};
