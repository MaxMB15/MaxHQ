// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Laying a run out with ELK. Nothing loads until a canvas first asks: then
// elkjs's API comes in, and its own worker file runs the layout off the
// main thread, as shipped. Each shape is laid out once, by its hash. Where
// there's no worker, as under tests, or the layout fails, the canvas gets
// the top-down column instead.

import { useEffect, useState } from "react";

import type { RunShape } from "@maxhq/shared/bindings";
import {
	type CanvasLayout,
	columnLayout,
	type ElkGraph,
	elkGraph,
	fromElk,
} from "@maxhq/shared/workflow/canvas";

type Elk = { readonly layout: (graph: ElkGraph) => Promise<ElkGraph> };

let elk: Promise<Elk> | null = null;

const start = (): Promise<Elk> =>
	(elk ??= Promise.all([
		import("elkjs/lib/elk-api.js"),
		import("elkjs/lib/elk-worker.min.js?url"),
	]).then(([api, worker]) => {
		const engine = new api.default({ workerUrl: worker.default });
		return {
			layout: (graph) =>
				engine.layout(graph as Parameters<typeof engine.layout>[0]) as Promise<ElkGraph>,
		};
	}));

/** Whether ELK can run here. */
const canRun = (): boolean => import.meta.env.MODE !== "test" && typeof Worker !== "undefined";

// The layouts by shape hash: done, and on their way. A few runs' worth.
const KEPT = 16;
const done = new Map<string, CanvasLayout>();
const coming = new Map<string, Promise<CanvasLayout>>();

const keep = (layout: CanvasLayout): CanvasLayout => {
	done.set(layout.hash, layout);
	coming.delete(layout.hash);
	for (const hash of done.keys()) {
		if (done.size <= KEPT) break;
		done.delete(hash);
	}
	return layout;
};

/** The shape's layout when it's already done, which under tests it always is. */
export const laidOut = (shape: RunShape): CanvasLayout | undefined =>
	done.get(shape.hash) ?? (canRun() ? undefined : keep(columnLayout(shape)));

/** The shape's layout, worked out once per hash. */
export const layOut = (shape: RunShape): Promise<CanvasLayout> => {
	const ready = laidOut(shape);
	if (ready) return Promise.resolve(ready);
	let pending = coming.get(shape.hash);
	if (!pending) {
		pending = start()
			.then((engine) => engine.layout(elkGraph(shape)))
			.then(
				(laid) => keep(fromElk(shape, laid)),
				(error: unknown) => {
					console.warn("Couldn't lay the run out, so it shows as a column", error);
					return keep(columnLayout(shape));
				}
			);
		coming.set(shape.hash, pending);
	}
	return pending;
};

/** The shape's layout, or null while ELK works it out. */
export const useCanvasLayout = (shape: RunShape): CanvasLayout | null => {
	const [layout, setLayout] = useState(() => laidOut(shape) ?? null);
	const current = layout?.hash === shape.hash ? layout : null;
	useEffect(() => {
		if (current) return;
		let live = true;
		void layOut(shape).then((laid) => {
			if (live) setLayout(laid);
		});
		return () => {
			live = false;
		};
	}, [shape, current]);
	return current ?? laidOut(shape) ?? null;
};
