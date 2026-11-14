// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// File drops, routed to the element under the pointer. Tauri's native drop
// carries real paths, which the webview's own drop events don't, but it
// reports one position for the whole window. Each composer registers its
// element and gets only the drops that land on it, plus a hover state
// while a drag is over it.
//
// On macOS, Tauri's drop also takes drags that start in the page, and
// reports them with no paths. Those aren't file drops. A target that asks
// for one gets what the drag carries, from page-drag.ts, and every other
// drag with no paths goes nowhere and lights nothing.

import { getCurrentWebview, type DragDropEvent } from "@tauri-apps/api/webview";
import { useEffect, useRef, useState } from "react";

import { createPageDrags, type PageDrag, type PageDrags } from "./page-drag";
import { inApp, onWindows } from "./platform";

export type { PageDrag };

/** What a drop target does with a drag. */
export type DropHandlers = {
	/**
	 * Takes the paths dropped on the element, and where they landed. Unset
	 * takes no files, such as a sidebar row that takes only quotes.
	 */
	readonly onDrop?: (paths: string[], at: Point) => void;
	/**
	 * Hears true when a drag moves over the element, and false when it
	 * moves off, drops or ends.
	 */
	readonly onHover?: (over: boolean) => void;
	/**
	 * Whether the element takes a drag from the page, such as a picture
	 * pulled out of a timeline, at that point. Unset takes none.
	 */
	readonly takesPageDrag?: (drag: PageDrag, at: Point) => boolean;
	/** Takes a drag from the page dropped where `takesPageDrag` said yes. */
	readonly onPageDrop?: (drag: PageDrag, at: Point) => void;
};

/** Set on a target while a drag is over it, for styling. */
export const DROP_OVER_ATTRIBUTE = "data-drop-over";

/** A point in CSS pixels from the webview's top left corner. */
export type Point = { readonly x: number; readonly y: number };

/** A drag event with its position in CSS pixels. */
export type DragEvent =
	| { readonly type: "enter"; readonly paths: string[]; readonly position: Point }
	| { readonly type: "over"; readonly position: Point }
	| { readonly type: "drop"; readonly paths: string[]; readonly position: Point }
	| { readonly type: "leave" };

/**
 * A drop position from Tauri in CSS pixels. Tauri calls it physical, but
 * on macOS and Linux it's already in points, which match CSS pixels. Only
 * Windows reports physical pixels, so only there does it divide by the
 * pixel ratio.
 */
export const toCssPoint = (position: Point, ratio: number, physical: boolean): Point =>
	physical && ratio > 0
		? { x: position.x / ratio, y: position.y / ratio }
		: { x: position.x, y: position.y };

/** Sends drag events to the registered element under the pointer. */
export type DropRouter = {
	/** Starts sending drops on `element` to `handlers`. Returns the undo. */
	readonly register: (element: Element, handlers: DropHandlers) => () => void;
	/** Routes one drag event. */
	readonly handle: (event: DragEvent) => void;
};

/** No drags from the page, for a router that only takes files. */
const NO_PAGE_DRAGS: Pick<PageDrags, "current" | "take" | "onEnd"> = {
	current: () => null,
	take: () => null,
	onEnd: () => () => {},
};

/**
 * A router that finds elements with `elementAt`. A drop on a child goes to
 * the nearest registered ancestor, and a drop on anything covering the
 * target, such as a dialog, goes nowhere. A drag with no paths is one
 * from the page, which goes only to a target that takes it.
 */
export const createDropRouter = (
	elementAt: (x: number, y: number) => Element | null,
	drags: Pick<PageDrags, "current" | "take" | "onEnd"> = NO_PAGE_DRAGS
): DropRouter => {
	const targets = new Map<Element, DropHandlers>();
	let over: Element | null = null;
	/** Whether the drag carries files. One Tauri hasn't described counts as files. */
	let files = true;

	const targetAt = (at: Point): Element | null => {
		let element = elementAt(at.x, at.y);
		while (element !== null && !targets.has(element)) element = element.parentElement;
		if (element === null) return null;
		if (files) return targets.get(element)?.onDrop ? element : null;
		const drag = drags.current();
		const takes = drag !== null && targets.get(element)?.takesPageDrag?.(drag, at) === true;
		return takes ? element : null;
	};

	const hover = (next: Element | null) => {
		if (next === over) return;
		const left = over;
		over = next;
		if (left !== null) {
			left.removeAttribute(DROP_OVER_ATTRIBUTE);
			targets.get(left)?.onHover?.(false);
		}
		if (next !== null) {
			next.setAttribute(DROP_OVER_ATTRIBUTE, "");
			targets.get(next)?.onHover?.(true);
		}
	};

	// A drag from the page that ends anywhere stops lighting its target.
	drags.onEnd(() => {
		if (!files) hover(null);
	});

	return {
		register: (element, handlers) => {
			targets.set(element, handlers);
			return () => {
				if (targets.get(element) !== handlers) return;
				if (over === element) hover(null);
				targets.delete(element);
			};
		},
		handle: (event) => {
			switch (event.type) {
				case "enter":
					files = event.paths.length > 0;
					hover(targetAt(event.position));
					break;
				case "over":
					hover(targetAt(event.position));
					break;
				case "drop": {
					files = event.paths.length > 0;
					const target = targetAt(event.position);
					const handlers = target === null ? undefined : targets.get(target);
					hover(null);
					if (files) handlers?.onDrop?.(event.paths, event.position);
					else {
						const drag = drags.take();
						if (drag !== null) handlers?.onPageDrop?.(drag, event.position);
					}
					files = true;
					break;
				}
				case "leave":
					hover(null);
					files = true;
			}
		},
	};
};

/** Tauri's drag event in CSS pixels. */
const fromTauri = (event: DragDropEvent): DragEvent => {
	if (event.type === "leave") return event;
	const position = toCssPoint(event.position, window.devicePixelRatio, onWindows());
	return event.type === "over"
		? { type: "over", position }
		: { type: event.type, paths: event.paths, position };
};

const pageDrags = createPageDrags();
const router = createDropRouter((x, y) => document.elementFromPoint(x, y), pageDrags);
let listening = false;

/** Listens to the window's drags once, the first time anything registers. */
const listen = () => {
	if (listening || !inApp()) return;
	listening = true;
	pageDrags.watch(document);
	getCurrentWebview()
		.onDragDropEvent((event) => router.handle(fromTauri(event.payload)))
		.catch(() => {
			listening = false;
		});
};

/**
 * Sends files dropped on `element` to `handlers`, and sets
 * `data-drop-over` on it while a drag is over it. Returns the undo. In a
 * browser, nothing drops.
 */
export const registerDropTarget = (element: Element, handlers: DropHandlers): (() => void) => {
	listen();
	return router.register(element, handlers);
};

/**
 * Takes the drag from the page for a drop the webview saw itself, and says
 * whether to go ahead with it. In the app, Tauri's drop may have taken the
 * drag already. In a browser, the webview's drop is the only one.
 */
export const claimPageDrop = (): boolean => !inApp() || pageDrags.take() !== null;

/**
 * Makes an element a drop target while it's mounted. Put `ref` on the
 * element. `over` says whether a drag is over it. `onDrop` may change on
 * every render.
 */
export const useDropTarget = <T extends Element>(
	onDrop: (paths: string[], at: Point) => void
): { ref: (element: T | null) => void; over: boolean } => {
	const [element, setElement] = useState<T | null>(null);
	const [over, setOver] = useState(false);
	const latest = useRef(onDrop);
	useEffect(() => {
		latest.current = onDrop;
	});
	useEffect(() => {
		if (element === null) return;
		return registerDropTarget(element, {
			onDrop: (paths, at) => latest.current(paths, at),
			onHover: setOver,
		});
	}, [element]);
	return { ref: setElement, over };
};
