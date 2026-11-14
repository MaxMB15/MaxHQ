// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// ⌘. stops an agent's turn, the Mac's key for cancel. The CLIs stop on Esc,
// but here Esc leaves a selected pill in the composer and closes menus and
// the find bar. The key goes to the agent whose pane focus is in. With focus
// outside every pane, such as in the sidebar, it goes to the active pane's.

import { useEffect, useLayoutEffect, useRef } from "react";

import { useServices } from "@/app/services";
import { panesOf } from "@/layout/panes";
import { holdsCommand, SHORTCUTS } from "@/layout/shortcuts";

const isStopKey = (e: KeyboardEvent): boolean =>
	holdsCommand(e) && !e.shiftKey && !e.altKey && e.code === SHORTCUTS.stop.code;

/**
 * Runs `stop` on ⌘. when the key is for the agent view at `pane`. Null
 * `stop` means there's no turn to stop, and the key passes on.
 */
export const useStopKey = (pane: Element | null, stop: (() => void) | null): void => {
	const services = useServices();
	const latest = useRef(stop);
	useLayoutEffect(() => {
		latest.current = stop;
	});
	const stoppable = stop !== null;

	useEffect(() => {
		if (!pane || !stoppable) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.defaultPrevented || e.isComposing || !isStopKey(e)) return;
			if (e.target instanceof Element && e.target.closest('[role="dialog"]')) return;
			const focus = document.activeElement;
			const api = panesOf(services).api();
			const inPane = (element: Element) => focus !== null && element.contains(focus);
			const mine =
				inPane(pane) ||
				(!!api &&
					!api.groups.some((group) => inPane(group.element)) &&
					!!api.activeGroup?.element.contains(pane));
			if (!mine) return;
			e.preventDefault();
			if (!e.repeat) latest.current?.();
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [pane, stoppable, services]);
};
