// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Drags that start in the page, such as a picture pulled out of a
// timeline. In the app on macOS, Tauri's drop takes every drag over the
// webview, the page's own too, so the page never sees its drop, and Tauri
// says only where the drag is, with no paths. What a drag from the page
// carries is noted here when it starts, and the drop router hands it to
// the target Tauri's drop lands on.
//
// Tauri's drop comes over IPC, so it can land after the drag has ended
// in the page. The drag stays a moment after it ends to be taken. A drag
// whose element went from the page may never hear its end, so the next
// press of the pointer ends it too.

/** What a drag from the page carries, by MaxHQ's own data types. */
export type PageDrag = ReadonlyMap<string, string>;

/** The data types a page drag is noted by. Anything else isn't MaxHQ's. */
const MAXHQ_TYPE = /^application\/x-maxhq-/;

/** How long a drag stays to be taken after it ends. */
export const LINGER_MS = 1000;

/** The drag in the air from the page, if it carries anything of MaxHQ's. */
export type PageDrags = {
	/** The drag in the air, or null. */
	readonly current: () => PageDrag | null;
	/** Takes the drag in the air, so nothing else gets it. */
	readonly take: () => PageDrag | null;
	/** Hears when a drag from the page ends, dropped or not. Returns the undo. */
	readonly onEnd: (listener: () => void) => () => void;
	/** Notes the drags that start in `root`. Once is enough. */
	readonly watch: (root: Document) => void;
};

/** Notes drags from the page, keeping each for `lingerMs` after it ends. */
export const createPageDrags = (lingerMs = LINGER_MS): PageDrags => {
	let drag: PageDrag | null = null;
	let timer: ReturnType<typeof setTimeout> | null = null;
	let watching = false;
	const listeners = new Set<() => void>();

	const stopTimer = () => {
		if (timer === null) return;
		clearTimeout(timer);
		timer = null;
	};

	const ended = () => {
		if (drag === null || timer !== null) return;
		timer = setTimeout(() => {
			timer = null;
			drag = null;
		}, lingerMs);
		for (const listener of [...listeners]) listener();
	};

	const started = (event: Event) => {
		stopTimer();
		const data = (event as DragEvent).dataTransfer;
		const carried = new Map<string, string>();
		for (const type of data?.types ?? []) {
			if (MAXHQ_TYPE.test(type)) carried.set(type, data?.getData(type) ?? "");
		}
		drag = carried.size > 0 ? carried : null;
		// An element that leaves the page mid drag hears its end only itself.
		if (drag !== null) event.target?.addEventListener("dragend", ended, { once: true });
	};

	return {
		current: () => drag,
		take: () => {
			const taken = drag;
			stopTimer();
			drag = null;
			return taken;
		},
		onEnd: (listener) => {
			listeners.add(listener);
			return () => listeners.delete(listener);
		},
		watch: (root) => {
			if (watching) return;
			watching = true;
			// Bubbling, so the element's own handler has set the data first.
			root.addEventListener("dragstart", started);
			root.addEventListener("dragend", ended);
			root.addEventListener("pointerdown", ended, true);
		},
	};
};
