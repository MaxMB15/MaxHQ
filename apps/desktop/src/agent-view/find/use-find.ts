// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// An agent view's find: asking maxd, lighting the matches on screen and
// going to the active one. Going there loads older pages until the item is
// in the timeline, scrolls its row into view, and then the match within
// the row and any box it scrolls in. A changing timeline asks again while
// the bar is open.

import { useEffect, useMemo } from "react";

import type { AgentId, FindMatch } from "@maxhq/shared/bindings";
import { createFind, type Find, type FindState } from "@maxhq/shared/find/store";
import { followTimeline, timelineAsk } from "@maxhq/shared/find/timeline";
import type { Timeline } from "@maxhq/shared/timeline/store";

import { useServices } from "@/app/services";

import { bringIntoView, type JumpTarget, seekItem, showsIn } from "../jump";
import { createFindPainter } from "./highlights";
import { registerFinder } from "./registry";
import { readFindPosition } from "./text";

// How far from a scrolling box's side a match it scrolls to sits, in pixels.
const MARGIN = 16;

/** Scrolls the code block or tool output a match sits in so the match shows. */
const scrollWithin = (range: Range) => {
	const box = range.startContainer.parentElement?.closest("pre");
	if (!box) return;
	const rect = range.getBoundingClientRect();
	const frame = box.getBoundingClientRect();
	if (box.scrollWidth > box.clientWidth) {
		if (rect.left < frame.left + MARGIN) box.scrollLeft += rect.left - frame.left - MARGIN;
		else if (rect.right > frame.right - MARGIN)
			box.scrollLeft += rect.right - frame.right + MARGIN;
	}
	if (box.scrollHeight > box.clientHeight && (rect.top < frame.top || rect.bottom > frame.bottom))
		box.scrollTop += rect.top - frame.top - Math.max(0, (frame.height - rect.height) / 2);
};

/** Lights the matches in `target`'s rows and goes to each match find moves to. Returns the undo. */
export const followFind = (find: Find, target: JumpTarget): (() => void) => {
	const painter = createFindPainter(target.scroller);
	const unread = find.readFrom((query) => readFindPosition(target.scroller, query));
	let stopSeek: (() => void) | null = null;
	const stop = () => {
		stopSeek?.();
		stopSeek = null;
	};

	const go = (query: string, match: FindMatch) => {
		stop();
		stopSeek = seekItem(
			target,
			match.item,
			(row) => {
				stopSeek = null;
				const range = painter.rangeIn(row, match);
				if (range) scrollWithin(range);
				const rect = (range ?? row).getBoundingClientRect();
				if (!showsIn(target.scroller, rect)) {
					target.hold();
					bringIntoView(target.scroller, rect);
				}
				painter.paint(query, match);
			},
			// The bar still says where the match is when its row can't be reached.
			() => {
				stopSeek = null;
			},
			// A match in a row on screen scrolls only as far as it needs.
			{ center: false }
		);
	};

	const apply = (state: FindState, before: FindState | null) => {
		const query = state.open ? state.query : "";
		if (!before || query !== (before.open ? before.query : "") || state.match !== before.match)
			painter.paint(query, state.match);
		if (!state.open) return stop();
		if (before && state.turn !== before.turn && state.match) go(query, state.match);
	};

	apply(find.store.getState(), null);
	const unsubscribe = find.store.subscribe(apply);
	return () => {
		unsubscribe();
		unread();
		stop();
		painter.dispose();
	};
};

/**
 * The find of an agent view whose section is `pane`, going to matches in
 * the list `target` names.
 */
export const useFind = (
	agent: AgentId,
	timeline: Timeline,
	pane: HTMLElement | null,
	target: JumpTarget | null
): Find => {
	const { source } = useServices();
	const find = useMemo(() => createFind(timelineAsk(source, agent)), [source, agent]);
	useEffect(() => find.dispose, [find]);

	useEffect(() => {
		if (pane) return registerFinder(pane, find.open);
	}, [pane, find]);

	useEffect(() => followTimeline(find, timeline), [timeline, find]);

	useEffect(() => {
		if (target) return followFind(find, target);
	}, [find, target]);

	return find;
};
