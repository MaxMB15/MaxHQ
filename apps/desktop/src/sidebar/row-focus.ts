// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sidebar's agent rows as one list for the keyboard. They take a single
// stop in the Tab order, Up and Down move focus from row to row across the
// projects and shelves, and Enter opens the focused agent, as each row is a
// button. While focus is on a row, that row keeps the stop; once focus
// leaves, the stop goes to the agent you're looking at. Rows come and go as
// agents change, so when the focused row goes, focus follows its agent to
// its new row, or else lands on the row that took its place. The summary
// row a caller's started agents collapse into is a row like any other.

const ROW = "[data-agent-row], [data-started-row]";

const idOf = (row: HTMLElement | undefined): string | undefined => {
	if (row?.dataset.agentRow !== undefined) return row.dataset.agentRow;
	const caller = row?.dataset.startedRow;
	return caller === undefined ? undefined : `started:${caller}`;
};

/** Runs the agent rows under `root` as one list, and returns a call that stops. */
export const watchRowFocus = (root: HTMLElement): (() => void) => {
	let rows: HTMLElement[] = [];
	// The agent whose row has focus, or had it last.
	let focused: string | undefined;
	// Whether focus is on a row, or was until that row went.
	let inside = false;

	const refresh = () => {
		const at = rows.findIndex((row) => idOf(row) === focused);
		rows = [...root.querySelectorAll<HTMLElement>(ROW)];
		const same = rows.find((row) => idOf(row) === focused);
		const looking = rows.find((row) => row.getAttribute("aria-current") === "true");
		const stop = inside
			? (same ?? rows[Math.min(Math.max(at, 0), rows.length - 1)])
			: (looking ?? same ?? rows[0]);
		for (const row of rows) {
			const index = row === stop ? 0 : -1;
			if (row.tabIndex !== index) row.tabIndex = index;
		}
		// A row that goes takes focus with it, to the page itself.
		const lost = document.activeElement === null || document.activeElement === document.body;
		if (inside && stop && lost) {
			focused = idOf(stop);
			stop.focus();
		}
	};

	const onFocus = (e: FocusEvent) => {
		const row = e.target instanceof HTMLElement && root.contains(e.target) ? e.target : null;
		inside = row?.matches(ROW) ?? false;
		if (inside) focused = idOf(row!);
		refresh();
	};
	// A click on something that takes no focus moves none, so it ends the
	// keyboard's hold too.
	const onPointer = () => {
		inside = false;
	};

	const onKey = (e: KeyboardEvent) => {
		if (e.defaultPrevented || e.isComposing || e.altKey || e.ctrlKey || e.metaKey) return;
		if (e.shiftKey || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
		const at = e.target instanceof HTMLElement ? rows.indexOf(e.target) : -1;
		if (at < 0) return;
		e.preventDefault();
		rows[e.key === "ArrowDown" ? at + 1 : at - 1]?.focus();
	};

	const observer = new MutationObserver(refresh);
	observer.observe(root, {
		childList: true,
		subtree: true,
		attributes: true,
		attributeFilter: ["data-agent-row", "data-started-row", "aria-current"],
	});
	document.addEventListener("focusin", onFocus);
	document.addEventListener("pointerdown", onPointer, true);
	root.addEventListener("keydown", onKey);
	refresh();
	return () => {
		observer.disconnect();
		document.removeEventListener("focusin", onFocus);
		document.removeEventListener("pointerdown", onPointer, true);
		root.removeEventListener("keydown", onKey);
	};
};
