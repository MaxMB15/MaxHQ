// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Draws a Mermaid diagram as an inert picture. Agents' text isn't trusted, so
// Mermaid draws off the page in its strictest mode with no way to set that
// from the text, DOMPurify cleans the drawing, and it shows only as an image.
// An SVG image runs no scripts and loads nothing, and its styles can't reach
// the app. Mermaid is several megabytes, so it loads with the first diagram
// that comes into view.
//
// Mermaid draws on this thread, so a diagram that never finishes stops the
// window. Two things limit that: a chart gets only so many steps through its
// dates, and a diagram that was drawing when the window last went away waits
// for a click, or a feed holding one could never open again.

import type { PluginFunc } from "dayjs";

import { currentTheme } from "@/app/theme";
import { hangGuard } from "@/lib/hang-guard";
import { fnv1a } from "@maxhq/shared/lib/fnv";
import type { Theme } from "@maxhq/shared/theme/theme";

import { DIAGRAM_CSS, diagramTheme } from "./diagram-theme";
import { t as translate } from "@maxhq/shared/i18n";

/** A diagram as a picture, or why it couldn't be drawn. */
export type Drawing =
	| {
			readonly state: "drawn";
			readonly url: string;
			/** Its size at 1:1, in CSS pixels. */
			readonly width: number;
			readonly height: number;
			/** The same picture larger, to see it whole. */
			readonly whole: () => string;
	  }
	| {
			readonly state: "failed";
			readonly reason: string;
			/** It wasn't tried, because of how the last try ended, and a click can ask again. */
			readonly heldBack?: boolean;
	  };

// Mermaid sizes a diagram such as a Gantt chart to what holds it, so this is
// the same width wherever it shows, and the picture scales from there.
const STAGE_WIDTH_PX = 760;

// How wide a picture is when seen whole, unless that would take it past 4x.
const WHOLE_WIDTH_PX = 1600;
const WHOLE_MOST = 4;

// A long session's diagrams are kept for scrolling back, up to this many.
const KEPT = 200;

const REASON_LENGTH = 160;

// A Gantt chart with `excludes` walks each task a day at a time, so a task of
// 250000 years would never finish. Each step adds to a date, and a diagram
// gets this many, which is over 500 years of days.
const MOST_DATE_STEPS = 200_000;

let stepsLeft = Number.POSITIVE_INFINITY;

const countDateSteps: PluginFunc = (_option, Dayjs) => {
	const add = Dayjs.prototype.add;
	Dayjs.prototype.add = function (...args) {
		stepsLeft -= 1;
		if (stepsLeft < 0) throw new Error(translate("The chart covers too long a time to draw."));
		return add.apply(this, args);
	};
};

// Which diagram is drawing, and those held back because one of them still was
// when the window went away, all as hashes of their text.
const guard = hangGuard("diagram");

const HELD_BACK_REASON = "MaxHQ stopped or closed while drawing it last time.";

const markOf = (source: string): string => fnv1a(source).toString(36);

type Tools = {
	readonly render: (id: string, text: string, holder: Element) => Promise<{ svg: string }>;
	readonly clean: (svg: string) => DocumentFragment;
};

const load = async (): Promise<Tools> => {
	const [{ default: mermaid }, { default: DOMPurify }, { default: dayjs }] = await Promise.all([
		import("mermaid"),
		import("dompurify"),
		import("dayjs"),
	]);
	dayjs.extend(countDateSteps);
	// Mermaid takes the app's colours again whenever the theme has changed.
	let look: Theme | null = null;
	const setUp = (theme: Theme) => {
		if (look === theme) return;
		look = theme;
		const styles = getComputedStyle(document.documentElement);
		mermaid.initialize({
			startOnLoad: false,
			securityLevel: "strict",
			suppressErrorRendering: true,
			htmlLabels: false,
			flowchart: { htmlLabels: false },
			theme: "base",
			themeVariables: diagramTheme((name) => styles.getPropertyValue(name).trim(), theme),
			themeCSS: DIAGRAM_CSS,
			// What a diagram's own `%%{init}%%` can't change.
			secure: [
				"secure",
				"securityLevel",
				"startOnLoad",
				"maxTextSize",
				"maxEdges",
				"suppressErrorRendering",
				"theme",
				"themeCSS",
				"fontFamily",
				"altFontFamily",
				"htmlLabels",
			],
		});
	};
	return {
		render: (id, text, holder) => {
			setUp(currentTheme());
			return mermaid.render(id, text, holder);
		},
		clean: (svg) =>
			DOMPurify.sanitize(svg, {
				USE_PROFILES: { svg: true, svgFilters: true },
				FORBID_TAGS: ["a", "script"],
				FORBID_ATTR: ["href", "xlink:href"],
				RETURN_DOM_FRAGMENT: true,
			}),
	};
};

let tools: Promise<Tools> | null = null;
let stage: HTMLElement | null = null;
let drawnCount = 0;

// Off the page but laid out, since Mermaid measures its text. It has the
// defaults an image starts from, so words fit the boxes measured for them.
const stageElement = (): HTMLElement => {
	if (stage?.isConnected) return stage;
	stage = document.createElement("div");
	stage.setAttribute("aria-hidden", "true");
	stage.style.cssText = `position:fixed;left:-10000px;top:0;width:${STAGE_WIDTH_PX}px;visibility:hidden;font:16px/normal sans-serif;letter-spacing:normal`;
	document.body.append(stage);
	return stage;
};

const svgUrl = (svg: string, width: number, height: number): string =>
	`data:image/svg+xml;charset=utf-8,${encodeURIComponent(
		svg.replace(/^<svg /, `<svg width="${width}" height="${height}" `)
	)}`;

/** The picture of a cleaned drawing, sized from what Mermaid gave its root. */
export const pictureOf = (root: SVGSVGElement): Drawing => {
	const box = (root.getAttribute("viewBox") ?? "").split(/[\s,]+/).map(Number);
	const attribute = root.getAttribute("width") ?? "";
	const wide =
		Number.parseFloat(root.style.maxWidth) ||
		(attribute.endsWith("%") ? 0 : Number.parseFloat(attribute)) ||
		box[2];
	const [boxWidth, boxHeight] = [box[2], box[3]];
	if (!wide || !boxWidth || !boxHeight)
		return { state: "failed", reason: translate("The drawing has no size.") };
	const width = Math.ceil(wide);
	const height = Math.ceil((wide * boxHeight) / boxWidth);
	root.removeAttribute("width");
	root.removeAttribute("height");
	root.style.maxWidth = "";
	const svg = new XMLSerializer().serializeToString(root);
	const scale = Math.max(1, Math.min(WHOLE_MOST, WHOLE_WIDTH_PX / width));
	return {
		state: "drawn",
		url: svgUrl(svg, width, height),
		width,
		height,
		whole: () => svgUrl(svg, Math.round(width * scale), Math.round(height * scale)),
	};
};

const draw = async (source: string): Promise<Drawing> => {
	const mark = markOf(source);
	if (guard.held(mark)) return { state: "failed", reason: HELD_BACK_REASON, heldBack: true };
	try {
		tools ??= load();
		const { render, clean } = await tools;
		guard.start(mark);
		stepsLeft = MOST_DATE_STEPS;
		const { svg } = await render(`diagram-${++drawnCount}`, source, stageElement());
		const root = clean(svg).querySelector("svg");
		return root
			? pictureOf(root)
			: { state: "failed", reason: translate("The drawing came out empty.") };
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		return { state: "failed", reason: (message.split("\n")[0] ?? "").slice(0, REASON_LENGTH) };
	} finally {
		stepsLeft = Number.POSITIVE_INFINITY;
		guard.finish(mark);
	}
};

// Kept by theme and text, since a drawing has its theme's colours.
const drawings = new Map<string, Drawing>();
const drawing = new Map<string, Promise<Drawing>>();
let lastDrawing: Promise<unknown> = Promise.resolve();

const keyOf = (source: string, theme: Theme = currentTheme()): string => `${theme}\n${source}`;

const nextTask = () => new Promise<void>((resolve) => setTimeout(resolve));

/** The drawing of a diagram in the theme now, if it's been drawn. */
export const drawnDiagram = (source: string): Drawing | undefined => drawings.get(keyOf(source));

/** Draws a diagram, or gives the drawing that's underway or done. It never rejects. */
export const drawDiagram = (source: string): Promise<Drawing> => {
	const theme = currentTheme();
	const key = keyOf(source, theme);
	const kept = drawings.get(key);
	if (kept) return Promise.resolve(kept);
	const underway = drawing.get(key);
	if (underway) return underway;
	// One at a time, since the steps and the mark are each for one diagram.
	// Each in a task of its own, so the window paints and takes in what's
	// streaming between two.
	const started = lastDrawing
		.then(nextTask)
		.then(() => draw(source))
		.then((result) => {
			drawing.delete(key);
			// Drawn after the theme changed, it has the other theme's colours.
			if (currentTheme() !== theme) return result;
			if (drawings.size >= KEPT) {
				const oldest = drawings.keys().next().value;
				if (oldest !== undefined) drawings.delete(oldest);
			}
			drawings.set(key, result);
			return result;
		});
	lastDrawing = started;
	drawing.set(key, started);
	return started;
};

/** Lets a held back diagram try again the next time it's asked for. */
export const allowDiagram = (source: string): void => {
	guard.allow(markOf(source));
	drawings.delete(keyOf(source, "light"));
	drawings.delete(keyOf(source, "dark"));
};
