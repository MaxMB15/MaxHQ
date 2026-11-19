// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Display math in Markdown, cut out of the text around it for a client whose
// Markdown view can't draw it, as the phone's can't. It reads `$$` blocks as
// the desktop's parser does: two or more `$` at the start of a line, up to
// three spaces in, with nothing after them that holds a `$`, and a line of at
// least as many `$` and nothing else to close it. One that never closes runs
// to the end of the text. Nothing inside fenced code is math. A formula on a
// line of its own, `$$x$$`, comes out too, though the desktop draws it in the
// line as inline math, since the phone draws no inline math at all.
//
// Only blocks at the top of the text count. One inside a quote is left in
// the text, and so is one a list item indents by four or more. One a list
// item indents by less comes out as if it were at the top.

/** A run of Markdown, or a formula to draw on its own lines. */
export type MathSegment =
	| { kind: "markdown"; text: string }
	/** `closed` is false for a block whose closing line hasn't come, which runs to the end. */
	| { kind: "math"; tex: string; closed: boolean };

// A fence's opening or closing line, its marks in the first group.
const FENCE = /^ {0,3}(`{3,}|~{3,})/;

// A math block's opening line: its indent, its `$` marks, and nothing after
// them with a `$` in it, which the desktop's parser drops.
const OPENING = /^( {0,3})(\${2,})[^$]*$/;

// A math block's closing line, its marks in the first group.
const CLOSING = /^ {0,3}(\${2,})[ \t]*$/;

// A formula on a line of its own between runs of the same number of `$`.
const ONE_LINE = /^ {0,3}(\${2,})([^$](?:.*[^$])?)\1[ \t]*$/;

/**
 * `text` cut into Markdown and the display math between it, in order. A text
 * with none comes back as one Markdown segment. Runs of Markdown with only
 * spaces in them are left out.
 */
export const mathSegments = (text: string): MathSegment[] => {
	if (!text.includes("$$")) return [{ kind: "markdown", text }];
	const segments: MathSegment[] = [];
	let markdown: string[] = [];
	let math: { lines: string[]; marks: number; indent: number } | null = null;
	let fence: string | null = null;

	const endMarkdown = () => {
		const run = markdown.join("\n");
		if (run.trim() !== "") segments.push({ kind: "markdown", text: run });
		markdown = [];
	};

	for (const line of text.replace(/\r\n?/g, "\n").split("\n")) {
		if (math) {
			const marks = CLOSING.exec(line)?.[1];
			if (marks && marks.length >= math.marks) {
				segments.push({ kind: "math", tex: math.lines.join("\n"), closed: true });
				math = null;
			} else {
				// A line gives up as much of its indent as the opening line had.
				const indent = /^ */.exec(line)?.[0].length ?? 0;
				math.lines.push(line.slice(Math.min(indent, math.indent)));
			}
			continue;
		}
		const run = FENCE.exec(line)?.[1];
		if (fence !== null) {
			// A fence closes on a line of at least as many of its marks and nothing else.
			if (run && run[0] === fence[0] && run.length >= fence.length && line.trim() === run)
				fence = null;
			markdown.push(line);
			continue;
		}
		if (run) {
			fence = run;
			markdown.push(line);
			continue;
		}
		const opening = OPENING.exec(line);
		if (opening) {
			endMarkdown();
			math = {
				lines: [],
				marks: opening[2]?.length ?? 2,
				indent: opening[1]?.length ?? 0,
			};
			continue;
		}
		const [, marks = "", formula = ""] = ONE_LINE.exec(line) ?? [];
		// One holding its own marks is inline math and text, as `$$a$$ b $$`.
		if (formula.trim() !== "" && !formula.includes(marks)) {
			endMarkdown();
			segments.push({ kind: "math", tex: formula.trim(), closed: true });
			continue;
		}
		markdown.push(line);
	}
	if (math) segments.push({ kind: "math", tex: math.lines.join("\n"), closed: false });
	else endMarkdown();
	return segments.length === 0 ? [{ kind: "markdown", text }] : segments;
};
