// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Remembers what was running when the window last went away, so it waits for
// a click the next time rather than stopping the window again. Something
// that runs on the window's thread, such as a diagram drawing or a visual's
// scripts, can loop forever, and a feed holding one could then never open.
// Each thing is a mark, such as a hash of its text. Its mark is stored while
// it runs, which for a visual is as long as it shows, and any mark still
// stored the next time a guard is used belonged to a run that never ended,
// so it's held back until allowed. A page that goes away on its own, as on
// a reload, ends its runs. So does a window that still answered when the
// app quit, which the app tells the first page of its next launch
// (crates/app/src/hang_watch.rs).

declare global {
	interface Window {
		/** Whether the window had stopped when the app last quit, set before the page's scripts. */
		__MAXHQ_LAST_STOPPED__?: boolean;
	}
}

const SHOWN_KEY = "maxhq.pageShown";

// Whether what ran when the app last quit ended fine. Only the launch's
// first page can tell, since the session's storage outlives a reload.
const lastQuitAnswering = (): boolean => {
	try {
		if (sessionStorage.getItem(SHOWN_KEY) !== null) return false;
		sessionStorage.setItem(SHOWN_KEY, "1");
	} catch {
		return false;
	}
	return window.__MAXHQ_LAST_STOPPED__ === false;
};

const quitAnswering = lastQuitAnswering();

const stored = (key: string): string | null => {
	try {
		return localStorage.getItem(key);
	} catch {
		return null;
	}
};

const store = (key: string, value: string | null): void => {
	try {
		if (value === null) localStorage.removeItem(key);
		else localStorage.setItem(key, value);
	} catch {
		// Without storage nothing is held back.
	}
};

const words = (value: string | null): string[] => (value ?? "").split(" ").filter(Boolean);

export type HangGuard = {
	/** Whether this mark's last run never ended, so it waits to be allowed. */
	readonly held: (mark: string) => boolean;
	/** Notes a run starting. Each start wants its own finish. */
	readonly start: (mark: string) => void;
	/** Notes a run that ended, or was let go of before it did. */
	readonly finish: (mark: string) => void;
	/** Lets a held back mark run again. */
	readonly allow: (mark: string) => void;
};

/**
 * A guard whose marks are stored under `maxhq.<name>.drawing`, the ones
 * running, and `maxhq.<name>.heldBack`, the most recent `most` held back.
 */
export const hangGuard = (name: string, most = 20): HangGuard => {
	const drawingKey = `maxhq.${name}.drawing`;
	const heldBackKey = `maxhq.${name}.heldBack`;
	let lookedForLeftovers = false;

	const heldBack = (): string[] => {
		const held = words(stored(heldBackKey));
		if (lookedForLeftovers) return held;
		lookedForLeftovers = true;
		const leftovers = words(stored(drawingKey));
		if (leftovers.length === 0) return held;
		store(drawingKey, null);
		if (quitAnswering) return held;
		const now = [...held, ...leftovers.filter((mark) => !held.includes(mark))].slice(-most);
		store(heldBackKey, now.join(" "));
		return now;
	};

	// A page that gets to say it's going away isn't stuck in what it runs.
	window.addEventListener("pagehide", (event) => {
		if (!event.persisted) store(drawingKey, null);
	});

	return {
		held: (mark) => heldBack().includes(mark),
		start: (mark) => {
			heldBack();
			store(drawingKey, [...words(stored(drawingKey)), mark].join(" "));
		},
		finish: (mark) => {
			const running = words(stored(drawingKey));
			const at = running.indexOf(mark);
			if (at < 0) return;
			running.splice(at, 1);
			store(drawingKey, running.length > 0 ? running.join(" ") : null);
		},
		allow: (mark) =>
			store(
				heldBackKey,
				heldBack()
					.filter((held) => held !== mark)
					.join(" ")
			),
	};
};
