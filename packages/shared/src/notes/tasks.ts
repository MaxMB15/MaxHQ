// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The task boxes in a note's Markdown, `- [ ]` and `- [x]`, which a tap in
// the rendered note ticks. A rendered view numbers its boxes in the order it
// draws them, so these count the same way: every task item in the text,
// nested and numbered ones too, but none inside fenced code.

import { listLineOf } from "./lists";

// A fence's opening or closing line, its marks in the first group.
const FENCE = /^ {0,3}(`{3,}|~{3,})/;

/** Where each task box's mark, the space or `x` between its brackets, sits in `markdown`. */
const marksOf = (markdown: string): number[] => {
	const marks: number[] = [];
	let fence: string | null = null;
	let start = 0;
	for (const line of markdown.split("\n")) {
		const run = FENCE.exec(line)?.[1];
		if (fence !== null) {
			// A fence closes on a line of at least as many of its marks and nothing else.
			if (run && run[0] === fence[0] && run.length >= fence.length && line.trim() === run)
				fence = null;
		} else if (run) {
			fence = run;
		} else {
			const item = listLineOf(line);
			if (item?.task) marks.push(start + item.indentEnd + item.marker.length + 1);
		}
		start += line.length + 1;
	}
	return marks;
};

/** How many task boxes `markdown` draws. */
export const taskCount = (markdown: string): number => marksOf(markdown).length;

/**
 * `markdown` with its task box at `index`, counting from 0, ticked or not.
 * An index past the last box changes nothing.
 */
export const setTask = (markdown: string, index: number, done: boolean): string => {
	const at = marksOf(markdown)[index];
	if (at === undefined) return markdown;
	return markdown.slice(0, at) + (done ? "x" : " ") + markdown.slice(at + 1);
};
