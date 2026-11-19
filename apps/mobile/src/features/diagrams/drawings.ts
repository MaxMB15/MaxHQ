// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Diagrams and formulas drawn for the feed. Mermaid is megabytes and KaTeX
// isn't small, so neither runs in a block's own web view. A hidden page for
// each, made the first time a block asks for one and kept, draws them one at
// a time, and this store keeps what it drew by a hash of the text, so a row
// that scrolls back finds its drawing done. The block then shows the drawing
// in a small web view of its own that runs no script.

import { useEffect, useMemo } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { fnv1a } from "@maxhq/shared/lib/fnv";

/** What a hidden page draws: Mermaid diagrams or KaTeX formulas. */
export type DrawKind = "diagram" | "math";

/** A drawing as a page's body at its own size, in CSS pixels, or why it couldn't be drawn. */
export type Drawing =
	| {
			readonly state: "drawn";
			readonly body: string;
			readonly width: number;
			readonly height: number;
	  }
	| { readonly state: "failed"; readonly reason: string };

/** One thing for a page to draw. A formula's `size` is its font size in pixels. */
export type DrawJob = {
	readonly key: string;
	readonly kind: DrawKind;
	readonly source: string;
	readonly size: number;
};

export type Drawings = {
	readonly drawn: ReadonlyMap<string, Drawing>;
	/** Waiting their turn, oldest first. */
	readonly waiting: readonly DrawJob[];
	/** What each page is drawing now. */
	readonly drawing: Readonly<Record<DrawKind, DrawJob | null>>;
	/** The pages asked for so far, which stay. */
	readonly pages: readonly DrawKind[];
};

// A long session's drawings are kept for scrolling back, up to this many.
const KEPT = 200;

/** What a drawing is kept by. With the length beside the hash, two sharing one is unlikely enough. */
export const drawingKey = (kind: DrawKind, source: string, size = 0): string =>
	`${kind}:${size}:${source.length}:${fnv1a(source)}`;

/** A store of drawings, with what the blocks and the pages do to it. */
export const createDrawings = () => {
	const store = createStore<Drawings>(() => ({
		drawn: new Map(),
		waiting: [],
		drawing: { diagram: null, math: null },
		pages: [],
	}));

	/** Asks for a drawing, unless it's drawn or on its way. */
	const ask = (kind: DrawKind, source: string, size = 0): void => {
		const key = drawingKey(kind, source, size);
		const now = store.getState();
		if (
			now.drawn.has(key) ||
			now.drawing[kind]?.key === key ||
			now.waiting.some((job) => job.key === key)
		)
			return;
		store.setState({
			waiting: [...now.waiting, { key, kind, source, size }],
			pages: now.pages.includes(kind) ? now.pages : [...now.pages, kind],
		});
	};

	/** The next thing for `kind`'s page to draw, or null while it's busy or has nothing. */
	const take = (kind: DrawKind): DrawJob | null => {
		const now = store.getState();
		if (now.drawing[kind]) return null;
		const job = now.waiting.find((waiting) => waiting.kind === kind);
		if (!job) return null;
		store.setState({
			waiting: now.waiting.filter((waiting) => waiting !== job),
			drawing: { ...now.drawing, [kind]: job },
		});
		return job;
	};

	/** Keeps what `kind`'s page drew for `key`. An answer for what it gave up on is dropped. */
	const settle = (kind: DrawKind, key: string, drawing: Drawing): void => {
		const now = store.getState();
		if (now.drawing[kind]?.key !== key) return;
		const drawn = new Map(now.drawn).set(key, drawing);
		const oldest = drawn.keys().next().value;
		if (drawn.size > KEPT && oldest !== undefined) drawn.delete(oldest);
		store.setState({ drawn, drawing: { ...now.drawing, [kind]: null } });
	};

	/** Gives up on what `kind`'s page is drawing, as failed for `reason`. */
	const giveUp = (kind: DrawKind, reason: string): void => {
		const job = store.getState().drawing[kind];
		if (job) settle(kind, job.key, { state: "failed", reason });
	};

	return { store, ask, take, settle, giveUp };
};

/** The app's drawings. */
export const drawings = createDrawings();

/**
 * A block's drawing, asked for while `wanted`: undefined until it's drawn
 * or has failed. One pushed out by newer ones is asked for again.
 */
export const useDrawing = (
	kind: DrawKind,
	source: string,
	size: number,
	wanted: boolean
): Drawing | undefined => {
	const key = useMemo(() => drawingKey(kind, source, size), [kind, source, size]);
	const drawing = useStore(drawings.store, (state) => state.drawn.get(key));
	useEffect(() => {
		if (wanted && !drawing) drawings.ask(kind, source, size);
	}, [wanted, drawing, kind, source, size]);
	return drawing;
};
