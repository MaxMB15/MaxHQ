// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A command's output with its terminal colour codes drawn as styled runs
// of text, a line at a time, so a view can show only its last lines.

import { type CSSProperties, Fragment } from "react";

import {
	type AnsiSpan,
	type AnsiStyle,
	ansiBlocks,
	paletteColor,
	parseAnsi,
} from "@maxhq/shared/timeline/ansi";

import { ansiToken } from "@/terminal/palette";

/** The output's lines, each as its styled runs. A last line break makes no line. */
export const ansiLines = (output: string): AnsiSpan[][] => {
	const body = output.replace(/\r?\n$/, "");
	return body === "" ? [] : ansiBlocks(parseAnsi(body), 1);
};

// The shared reader names the 16 colours by their dark values. Draw them
// with the theme's terminal tokens, so they read in light too.
const THEMED = new Map(Array.from({ length: 16 }, (_, i) => [paletteColor(i), ansiToken(i)]));
const themed = (color: string): string => THEMED.get(color) ?? color;

const styleOf = (style: AnsiStyle): CSSProperties | undefined => {
	const css: CSSProperties = {};
	if (style.fg) css.color = themed(style.fg);
	if (style.bg) css.backgroundColor = themed(style.bg);
	if (style.bold) css.fontWeight = 600;
	if (style.dim) css.opacity = 0.6;
	if (style.italic) css.fontStyle = "italic";
	if (style.underline) css.textDecoration = "underline";
	return Object.keys(css).length > 0 ? css : undefined;
};

/** Lines of output in their colours, one line break between each. */
export const AnsiLines = ({ lines }: { lines: readonly AnsiSpan[][] }) =>
	lines.map((spans, line) => (
		// Lines and runs have no identity but their place in the output.
		<Fragment key={line}>
			{line > 0 ? "\n" : null}
			{spans.map((span, i) => {
				const style = styleOf(span.style);
				return style ? (
					<span key={i} style={style}>
						{span.text}
					</span>
				) : (
					<Fragment key={i}>{span.text}</Fragment>
				);
			})}
		</Fragment>
	));
