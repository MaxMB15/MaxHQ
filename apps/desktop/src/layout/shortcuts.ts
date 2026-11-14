// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The pane, sidebar, inbox and find shortcuts and how buttons and menus show them.
// They hold Command on a Mac and Control elsewhere. None is a key the composer
// edits text with or one the macOS menu bar takes, which is why closing a
// pane is Shift+W and not W, and maximizing is Shift+M and not M. Undo is
// the one exception: it undoes an inbox change only while its notice shows
// and no text field has focus, so the composer keeps its own undo.

import { openNextWaiting } from "@/app/open-agent";
import type { Services } from "@/app/services";
import { onMac } from "@/native/platform";
import { finderFor } from "@/agent-view/find/registry";
import { toggleShortcutList } from "@/shortcut-list/store";
import { canUndoInbox, settleAndMoveOn, togglePinChosen, undoInbox } from "@/sidebar/commands";
import { focusFilter } from "@/sidebar/filter";
import { toggleSidebar } from "@/sidebar/toggle";

import {
	closePane,
	cycleTab,
	type FocusDirection,
	focusGroup,
	showTab,
	splitGroup,
	toggleMaximized,
} from "./commands";
import { panesOf } from "./panes";

/** A key with Command, or Control off a Mac. */
export type Shortcut = {
	/** The key as a menu shows it. */
	readonly label: string;
	/** The key's `KeyboardEvent.code`. */
	readonly code: string;
	readonly shift?: boolean;
	readonly alt?: boolean;
};

const ARROWS: Record<FocusDirection, Shortcut> = {
	left: { label: "←", code: "ArrowLeft", alt: true },
	right: { label: "→", code: "ArrowRight", alt: true },
	up: { label: "↑", code: "ArrowUp", alt: true },
	down: { label: "↓", code: "ArrowDown", alt: true },
};

/** Every pane, sidebar, inbox and find shortcut but the numbered tabs. */
export const SHORTCUTS = {
	nextTab: { label: "]", code: "BracketRight", shift: true },
	previousTab: { label: "[", code: "BracketLeft", shift: true },
	splitRight: { label: "D", code: "KeyD" },
	splitDown: { label: "D", code: "KeyD", shift: true },
	maximize: { label: "M", code: "KeyM", shift: true },
	close: { label: "W", code: "KeyW", shift: true },
	reopen: { label: "T", code: "KeyT", shift: true },
	nextWaiting: { label: "J", code: "KeyJ" },
	settle: { label: "S", code: "KeyS", shift: true },
	pin: { label: "P", code: "KeyP", shift: true },
	undo: { label: "Z", code: "KeyZ" },
	find: { label: "F", code: "KeyF" },
	sidebar: { label: "B", code: "KeyB" },
	filter: { label: "F", code: "KeyF", alt: true },
	// Shown only here: the new-agent form listens for its own key.
	newAgent: { label: "N", code: "KeyN" },
	stop: { label: ".", code: "Period" },
	shortcuts: { label: "/", code: "Slash" },
} as const satisfies Record<string, Shortcut>;

/** How a tooltip or menu writes a shortcut, such as ⇧⌘D on a Mac or Ctrl+Shift+D elsewhere. */
export const keyHint = (s: Shortcut): string =>
	onMac()
		? `${s.alt ? "⌥" : ""}${s.shift ? "⇧" : ""}⌘${s.label}`
		: `Ctrl+${s.alt ? "Alt+" : ""}${s.shift ? "Shift+" : ""}${s.label}`;

/** A shortcut in the form `aria-keyshortcuts` takes. */
export const ariaKeys = (s: Shortcut): string =>
	`${onMac() ? "Meta" : "Control"}+${s.alt ? "Alt+" : ""}${s.shift ? "Shift+" : ""}${s.label}`;

/**
 * Whether a key or click holds Command alone on a Mac, where Control moves
 * the cursor in the composer and Control-click opens a menu, or Control
 * alone elsewhere.
 */
export const holdsCommand = (e: {
	readonly metaKey: boolean;
	readonly ctrlKey: boolean;
}): boolean => (onMac() ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey);

// Letters go by the letter they type, so Command and D means the key marked
// D on any Latin layout. A layout that types no Latin letter goes by where
// the key sits, and so do the other keys.
const matches = (e: KeyboardEvent, s: Shortcut): boolean => {
	if (!holdsCommand(e) || !!s.shift !== e.shiftKey || !!s.alt !== e.altKey) return false;
	const typed = e.key.toLowerCase();
	if (s.code.startsWith("Key") && /^[a-z]$/.test(typed)) {
		return typed === s.code.slice(3).toLowerCase();
	}
	return e.code === s.code;
};

// Command and a digit from 1 to 9, which shows that tab.
const tabPlace = (e: KeyboardEvent): number | null => {
	if (!holdsCommand(e) || e.shiftKey || e.altKey) return null;
	const match = /^Digit([1-9])$/.exec(e.code);
	return match ? Number(match[1]) : null;
};

// Typing carries on in the pane a shortcut moved to, once it has drawn.
const focusComposer = (services: Services): void => {
	setTimeout(() => {
		const group = panesOf(services).api()?.activeGroup;
		group?.element.querySelector<HTMLElement>('[contenteditable="true"]')?.focus();
	}, 0);
};

// Whether focus is in a field you type in, which keeps its own undo.
const typing = (): boolean => {
	const at = document.activeElement;
	if (!(at instanceof HTMLElement)) return false;
	return at.isContentEditable || at.matches("input, textarea, select");
};

// What a key press does, or null when it isn't a shortcut. Holding a key
// down moves focus or tabs on, and does the rest only once. A command that
// returns true moves focus to the composer of the pane it showed.
const commandFor = (services: Services, e: KeyboardEvent): (() => boolean) | null => {
	const panes = panesOf(services);
	const once = (run: () => boolean) => (e.repeat ? () => false : run);
	// Runs without moving focus.
	const stay = (run: () => void) => () => {
		run();
		return false;
	};
	if (matches(e, SHORTCUTS.shortcuts)) return once(stay(() => toggleShortcutList(services)));
	if (matches(e, SHORTCUTS.nextWaiting)) return once(() => openNextWaiting(services));
	if (matches(e, SHORTCUTS.reopen)) return once(panes.reopen);
	// The sidebar's keys work in either mode, and the filter's shows the projects.
	if (matches(e, SHORTCUTS.sidebar)) return once(stay(() => toggleSidebar(services)));
	if (matches(e, SHORTCUTS.filter)) return once(stay(() => focusFilter(services)));
	// The inbox shortcuts work in either mode. Undo passes the key on when
	// there's nothing to undo or you're typing.
	if (matches(e, SHORTCUTS.settle)) return once(() => settleAndMoveOn(services));
	if (matches(e, SHORTCUTS.pin)) return once(stay(() => togglePinChosen(services)));
	if (matches(e, SHORTCUTS.undo)) {
		return canUndoInbox(services) && !typing() ? once(stay(() => undoInbox(services))) : null;
	}
	// The panes are hidden in devices mode. The first two shortcuts above
	// open a pane, which brings them back, and the inbox ones need no pane,
	// so only those work there.
	const api = panes.api();
	if (!api || services.ui.getState().mode !== "projects") return null;
	// Find opens in the agent view focus is in, or the active pane's.
	if (matches(e, SHORTCUTS.find)) {
		const open = finderFor(document.activeElement, api.activeGroup?.element ?? null);
		return open ? once(stay(open)) : null;
	}
	for (const [direction, s] of Object.entries(ARROWS) as [FocusDirection, Shortcut][]) {
		if (matches(e, s)) return () => focusGroup(api, direction);
	}
	if (matches(e, SHORTCUTS.nextTab)) return () => cycleTab(api, 1);
	if (matches(e, SHORTCUTS.previousTab)) return () => cycleTab(api, -1);
	const place = tabPlace(e);
	if (place) return () => showTab(api, place);
	if (matches(e, SHORTCUTS.splitRight))
		return once(() => splitGroup(api, api.activeGroup, "right"));
	if (matches(e, SHORTCUTS.splitDown))
		return once(() => splitGroup(api, api.activeGroup, "below"));
	if (matches(e, SHORTCUTS.maximize)) return once(() => toggleMaximized(api.activeGroup));
	if (matches(e, SHORTCUTS.close)) return once(() => closePane(api));
	return null;
};

/**
 * Listens for the pane shortcuts across the window, and returns a call that
 * stops. A key the composer or an open dialog already took is left alone.
 */
export const watchShortcuts = (services: Services): (() => void) => {
	const onKey = (e: KeyboardEvent) => {
		if (e.defaultPrevented || e.isComposing) return;
		if (e.target instanceof Element && e.target.closest('[role="dialog"]')) return;
		const command = commandFor(services, e);
		if (!command) return;
		e.preventDefault();
		if (command()) focusComposer(services);
	};
	window.addEventListener("keydown", onKey);
	return () => window.removeEventListener("keydown", onKey);
};
