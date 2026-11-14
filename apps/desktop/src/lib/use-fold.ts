// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Keeps a row of controls on one line by folding it a step at a time, with
// `fitLevel` picking the level. A ResizeObserver watches the row. A row
// that states how wide each level is folds by its width alone. Any other
// row measures itself after every render: a level that runs past the
// row's edge notes how wide it wanted to be, and the row folds one more
// step. What it noted goes stale when what the row shows changes. Either
// way, a new level renders before the browser paints, so the row never
// shows a frame at the wrong one.

import { fitLevel } from "@maxhq/shared/ui/fold";
import {
	type RefObject,
	useLayoutEffect,
	useReducer,
	useRef,
	useState,
	useSyncExternalStore,
} from "react";

/**
 * How a row folds: by the width each level `needs`, from none folded to
 * all of them, or by measuring its `steps`, with `content` naming what it
 * shows, such as its labels joined.
 */
export type Fold =
	{ readonly needs: readonly number[] } | { readonly steps: number; readonly content: string };

// The level lives outside React's state, so a change the ResizeObserver
// makes renders before the next paint, as a store's change does, rather
// than after it, as a state update from outside React would. React listens
// only once the row has mounted, so `set` says when it had no one to tell.
const levelStore = () => {
	let level = 0;
	let changed: (() => void) | null = null;
	return {
		get: () => level,
		set: (next: number): boolean => {
			if (next === level) return false;
			level = next;
			changed?.();
			return changed === null;
		},
		subscribe: (listener: () => void) => {
			changed = listener;
			return () => {
				changed = null;
			};
		},
	};
};

/** How many steps the row in `row` folds, see `Fold`. */
export const useFold = (row: RefObject<HTMLElement | null>, fold: Fold): number => {
	const [store] = useState(levelStore);
	const level = useSyncExternalStore(store.subscribe, store.get, store.get);
	const [, rerender] = useReducer((count: number) => count + 1, 0);
	const steps = "needs" in fold ? fold.needs.length - 1 : fold.steps;
	const reported = useRef<number | null>(null);
	const measured = useRef<(number | undefined)[]>([]);
	const shown = useRef<string | null>(null);
	const fit = useRef(() => {});
	useLayoutEffect(() => {
		const key = "needs" in fold ? null : `${fold.steps}:${fold.content}`;
		if (key !== shown.current) {
			shown.current = key;
			measured.current = Array.from({ length: steps + 1 }, () => undefined);
		}
		fit.current = () => {
			const node = row.current;
			if (!node) return;
			const at = Math.min(store.get(), steps);
			if ("needs" in fold) {
				if (reported.current === null) return;
				if (store.set(fitLevel(reported.current, fold.needs, at))) rerender();
				return;
			}
			const width = node.clientWidth;
			// A hidden row has no width, and folds when it shows again.
			if (width === 0) return;
			if (node.scrollWidth > width) measured.current[at] = node.scrollWidth;
			if (store.set(fitLevel(width, measured.current, at))) rerender();
		};
		fit.current();
	});
	useLayoutEffect(() => {
		const node = row.current;
		if (!node) return;
		const observer = new ResizeObserver(([entry]) => {
			if (entry) reported.current = entry.contentRect.width;
			fit.current();
		});
		observer.observe(node);
		return () => observer.disconnect();
	}, [row]);
	return Math.min(level, steps);
};
