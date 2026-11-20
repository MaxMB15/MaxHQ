// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The window's live terminals, outside React. The timeline is virtualized
// and the drawer's tabs come and go, so each terminal has one view that
// lives here, and a component shows it by placing the view's element in a
// host of its own and taking it out again. The view keeps its screen and
// scrollback all the while.
//
// The first place attaches to the terminal's output, from where the view
// ends, and the last one gone detaches it a while later, so a row that
// scrolls past or a tab you flick through doesn't attach again each time.
// A reset or a gap in the offsets clears the view and goes on from where
// the device starts over. At most eight views draw with the GPU, the one
// with focus first and then the ones shown most lately. The rest, and any
// whose GPU context dropped, draw with the DOM.

import type { TerminalId } from "@maxhq/shared/bindings";
import type { TerminalSize } from "@maxhq/shared/terminal/requests";
import {
	lastByte,
	stepFor,
	type TerminalEvent,
	type TerminalStream,
} from "@maxhq/shared/terminal/stream";

import type { MakeView, TermView } from "./term-view";

/** How long a terminal stays attached after the last place it showed in goes. */
export const DETACH_AFTER = 30_000;

/** How many views draw with the GPU at once. */
export const GPU_BUDGET = 8;

/** How a host shows a terminal. */
export type Placement = {
	/** Whether keys typed in it go to the terminal, and its size to the device. */
	readonly interactive: boolean;
	/**
	 * The most rows it takes, as a card in the timeline does. It takes fewer
	 * while the output is shorter, at the terminal's own width. With none it
	 * fills the host.
	 */
	readonly maxRows?: number;
};

export type RegistryOptions = {
	readonly stream: TerminalStream;
	readonly makeView: MakeView;
	/** The terminal's size on its device, or null when the view doesn't have it. */
	readonly sizeOf: (terminal: TerminalId) => TerminalSize | null;
	/** Whether the terminal is still there, so a view of one that closed goes. */
	readonly exists: (terminal: TerminalId) => boolean;
	/**
	 * Whether the OSC 133 in the terminal's output are its shell's marks, as
	 * the device says. With none, no view takes any.
	 */
	readonly marksOf?: (terminal: TerminalId) => boolean;
	/** Hears that an attach failed, with a sentence to show. */
	readonly onFailure?: (terminal: TerminalId, message: string) => void;
	readonly detachAfter?: number;
	readonly gpuBudget?: number;
};

/** The window's live terminals. */
export type Registry = {
	/**
	 * Shows the terminal in `host` until the returned call. A later place
	 * takes the view from an earlier one, which gets it back when the later
	 * one goes.
	 */
	readonly place: (terminal: TerminalId, host: HTMLElement, placement: Placement) => () => void;
	/** Sizes the view to its host again, as after the host changed size. */
	readonly fit: (terminal: TerminalId) => void;
	/** The terminal's view, while this window holds one. */
	readonly view: (terminal: TerminalId) => TermView | null;
	/** The terminal whose view has focus, if any. */
	readonly focused: () => TerminalId | null;
	/**
	 * Types `text` as a paste, which a shell that asks for it gets bracketed,
	 * once the terminal has printed something, such as its prompt, and shows
	 * somewhere.
	 */
	readonly paste: (terminal: TerminalId, text: string) => void;
	/** Lets go of a terminal that closed, at once. */
	readonly forget: (terminal: TerminalId) => void;
	readonly dispose: () => void;
};

type Spot = {
	readonly host: HTMLElement;
	readonly placement: Placement;
	observer: ResizeObserver | null;
};

type Entry = {
	readonly terminal: TerminalId;
	readonly view: TermView;
	/** Where the view's output ends, in all the terminal printed. */
	end: number | null;
	/** Where it shows, the last one holding the element. */
	readonly spots: Spot[];
	attached: boolean;
	timer: ReturnType<typeof setTimeout> | null;
	focused: boolean;
	lost: boolean;
	/** When it last showed or took focus, for the GPU's order. */
	used: number;
	/** The size this window last told the device. */
	sent: TerminalSize | null;
	/** Pastes waiting for the terminal's first output. */
	pastes: string[];
	readonly stops: (() => void)[];
};

const encoder = new TextEncoder();

export const createRegistry = ({
	stream,
	makeView,
	sizeOf,
	exists,
	marksOf = () => false,
	onFailure,
	detachAfter = DETACH_AFTER,
	gpuBudget = GPU_BUDGET,
}: RegistryOptions): Registry => {
	const entries = new Map<TerminalId, Entry>();
	let clock = 0;

	const top = (entry: Entry): Spot | undefined => entry.spots.at(-1);

	// The focused view first, then the ones shown most lately.
	const rank = () => {
		const shown = [...entries.values()]
			.filter((entry) => entry.spots.length > 0 && !entry.lost)
			.sort((a, b) => Number(b.focused) - Number(a.focused) || b.used - a.used);
		const gpu = new Set(shown.slice(0, gpuBudget));
		for (const entry of entries.values()) entry.view.setGpu(gpu.has(entry));
	};

	const layout = (entry: Entry) => {
		const spot = top(entry);
		if (!spot) return;
		const { maxRows, interactive } = spot.placement;
		if (maxRows !== undefined) {
			const cols = sizeOf(entry.terminal)?.cols ?? entry.view.size().cols;
			entry.view.resize(cols, Math.max(1, Math.min(maxRows, entry.view.usedRows())));
			return;
		}
		const size = entry.view.fit();
		if (!size || !interactive) return;
		if (size.cols === entry.sent?.cols && size.rows === entry.sent.rows) return;
		entry.sent = size;
		stream.resize(entry.terminal, size);
	};

	const flushPastes = (entry: Entry) => {
		if (!entry.end || !top(entry)?.placement.interactive) return;
		for (const text of entry.pastes.splice(0)) entry.view.paste(text);
	};

	const show = (entry: Entry) => {
		const spot = top(entry);
		if (!spot) return entry.view.element.remove();
		if (entry.view.element.parentElement !== spot.host) spot.host.append(entry.view.element);
		entry.view.setInteractive(spot.placement.interactive);
		layout(entry);
		flushPastes(entry);
	};

	const onEvent = (entry: Entry) => (event: TerminalEvent) => {
		const step = stepFor(entry.end, event);
		if (!step) return;
		if (step.clear) entry.view.clear();
		entry.view.setMarks(marksOf(entry.terminal));
		if (step.bytes.length > 0) entry.view.write(step.bytes, step.end - step.bytes.length);
		entry.end = step.end;
		if (top(entry)?.placement.maxRows !== undefined) layout(entry);
		flushPastes(entry);
	};

	const entryFor = (terminal: TerminalId): Entry => {
		const found = entries.get(terminal);
		if (found) return found;
		const view = makeView();
		const entry: Entry = {
			terminal,
			view,
			end: null,
			spots: [],
			attached: false,
			timer: null,
			focused: false,
			lost: false,
			used: 0,
			sent: null,
			pastes: [],
			stops: [],
		};
		entry.stops.push(
			stream.listen(terminal, onEvent(entry)),
			// What you type goes straight on, and nothing here keeps it.
			view.onData((data) => {
				if (top(entry)?.placement.interactive) stream.input(terminal, encoder.encode(data));
			}),
			view.onFocus((focused) => {
				entry.focused = focused;
				if (focused) entry.used = ++clock;
				rank();
			}),
			view.onGpuLost(() => {
				entry.lost = true;
				rank();
			})
		);
		entries.set(terminal, entry);
		return entry;
	};

	const attach = (entry: Entry) => {
		if (entry.attached) return;
		entry.attached = true;
		stream.attach(entry.terminal, lastByte(entry.end)).catch((error: unknown) => {
			entry.attached = false;
			onFailure?.(entry.terminal, error instanceof Error ? error.message : String(error));
		});
	};

	const drop = (entry: Entry) => {
		if (entry.timer) clearTimeout(entry.timer);
		for (const spot of entry.spots) spot.observer?.disconnect();
		if (entry.attached) stream.detach(entry.terminal);
		for (const stop of entry.stops) stop();
		entry.view.dispose();
		entries.delete(entry.terminal);
	};

	const release = (entry: Entry) => {
		entry.timer = null;
		if (entry.spots.length > 0) return;
		if (!exists(entry.terminal)) return drop(entry);
		if (entry.attached) stream.detach(entry.terminal);
		entry.attached = false;
	};

	return {
		place: (terminal, host, placement) => {
			const entry = entryFor(terminal);
			if (entry.timer) clearTimeout(entry.timer);
			entry.timer = null;
			const spot: Spot = { host, placement, observer: null };
			if (typeof ResizeObserver !== "undefined") {
				spot.observer = new ResizeObserver(() => {
					if (top(entry) === spot) layout(entry);
				});
				spot.observer.observe(host);
			}
			entry.spots.push(spot);
			entry.used = ++clock;
			show(entry);
			attach(entry);
			rank();
			return () => {
				spot.observer?.disconnect();
				const at = entry.spots.indexOf(spot);
				if (at < 0) return;
				const wasTop = at === entry.spots.length - 1;
				entry.spots.splice(at, 1);
				if (wasTop) show(entry);
				if (entry.spots.length === 0 && entries.get(terminal) === entry) {
					entry.focused = false;
					entry.timer = setTimeout(() => release(entry), detachAfter);
				}
				rank();
			};
		},
		fit: (terminal) => {
			const entry = entries.get(terminal);
			if (entry) layout(entry);
		},
		view: (terminal) => entries.get(terminal)?.view ?? null,
		focused: () => [...entries.values()].find((entry) => entry.focused)?.terminal ?? null,
		paste: (terminal, text) => {
			// A tab that opened for the paste shows a moment later.
			const entry = entryFor(terminal);
			entry.pastes.push(text);
			flushPastes(entry);
		},
		forget: (terminal) => {
			const entry = entries.get(terminal);
			if (!entry) return;
			drop(entry);
			rank();
		},
		dispose: () => {
			for (const entry of [...entries.values()]) drop(entry);
		},
	};
};
