// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The two kinds of page behind a diagram or a formula. A drawing page holds
// Mermaid or KaTeX, as a visual's page holds its libraries, and draws what
// the app sends it: Mermaid in the desktop's strict settings and colours,
// then cleaned as the desktop cleans it, and KaTeX as MathML, which the web
// view draws in its own math font, so no font comes with it. A shown page
// holds one drawing and runs no script: a diagram as an SVG picture, which
// runs nothing and loads nothing, as on the desktop, or a formula's markup.

import { t } from "@maxhq/shared/i18n";

import { diagramTheme } from "@maxhq/shared/agent-view/diagram-theme";
import {
	drawingSize,
	failureReason,
	mathSettings,
	mermaidSettings,
	STAGE_WIDTH_PX,
	svgUrl,
} from "@maxhq/shared/agent-view/drawing";
import { LIBRARY_PREFIX } from "@maxhq/shared/visuals/frame";
import { inlineVisualDocument, readBridgeReport } from "@maxhq/shared/visuals/inline";

import { color } from "../../theme";
import type { DrawJob, DrawKind, Drawing } from "./drawings";

/** The desktop's dark tokens a diagram draws with, in the phone's colours. */
export const DIAGRAM_TOKENS: Readonly<Record<string, string>> = {
	"--foreground": color.foreground,
	"--muted-foreground": color.mutedForeground,
	"--subtle-foreground": color.subtleForeground,
	"--subpanel": color.subpanel,
	"--popover": color.secondary,
	"--card": color.card,
	"--border": color.border,
	"--on-series": color.canvas,
	"--series-0": color.primary,
	"--series-4": color.visual,
	// The desktop's dark tokens a diagram draws with that the phone's theme doesn't name.
	"--hover-card": "#1e242e",
	"--hover-card-border": "#343b47",
	"--series-1": "#d95926",
	"--series-2": "#199e70",
	"--series-3": "#c98500",
	"--series-5": "#9085e9",
	"--series-6": "#e66767",
	"--series-7": "#2aa5c0",
};

const token = (name: string): string => DIAGRAM_TOKENS[name] ?? "";

// A formula's look, the same on both pages so it measures as it shows. The
// page's base look sets a taller line, and MathML's annotation holds the
// source, which only a reader wants.
const MATH_CSS = `.math{display:inline-block;width:max-content;white-space:nowrap;color:${color.foreground};line-height:normal;letter-spacing:normal}.math .katex-display{display:block;margin:0}.math annotation{display:none}`;

// Laid out but out of sight, as on the desktop: Mermaid measures its text.
const STAGE_CSS = `#stage{position:absolute;left:0;top:0;width:${STAGE_WIDTH_PX}px;font:16px/normal sans-serif;letter-spacing:normal}#box{position:absolute;left:0;top:0}`;

// What both drawing pages share. A job comes as `window.maxhqDraw(job)`, and
// each answer carries the job's key. A library that didn't load fails each
// job, so the page still says it's ready.
const BRIDGE = `var tell = function (answer) { window.ReactNativeWebView.postMessage(JSON.stringify(answer)); };
var broken = null;
var reason = function (error) { return error && error.message ? String(error.message) : String(error); };
var draw = function () {};
try { setUp(); } catch (error) { broken = error; }
window.maxhqDraw = function (job) {
	var answer = function (fields) { fields.key = job.key; tell(fields); };
	var failed = function (error) { answer({ type: "failed", reason: reason(error) }); };
	if (broken) return failed(broken);
	try { draw(job, answer, failed); } catch (error) { failed(error); }
};
tell({ type: "ready" });`;

// Draws with Mermaid on the stage, then keeps what the desktop's cleaning
// keeps: no scripts, links or HTML inside the picture, and nothing that acts.
const DIAGRAM_DRAWER = `var setUp = function () {
	mermaid.initialize(SETTINGS);
	var stage = document.getElementById("stage");
	var count = 0;
	var each = function (list, run) { Array.prototype.slice.call(list).forEach(run); };
	var clean = function (svg) {
		var holder = document.createElement("template");
		holder.innerHTML = svg;
		var root = holder.content.querySelector("svg");
		if (!root) return null;
		each(root.querySelectorAll("script, foreignObject"), function (node) { node.remove(); });
		each(root.querySelectorAll("a"), function (link) { link.replaceWith.apply(link, Array.prototype.slice.call(link.childNodes)); });
		each([root].concat(Array.prototype.slice.call(root.querySelectorAll("*"))), function (node) {
			each(node.attributes, function (attribute) {
				var name = attribute.name.toLowerCase();
				if (name.indexOf("on") === 0 || name === "href" || name === "xlink:href") node.removeAttribute(attribute.name);
			});
		});
		return root;
	};
	draw = function (job, answer, failed) {
		count += 1;
		mermaid.render("diagram-" + count, job.source, stage).then(function (drawn) {
			stage.textContent = "";
			var root = clean(drawn.svg);
			if (!root) return answer({ type: "empty" });
			var maxWidth = root.style.maxWidth;
			var width = root.getAttribute("width");
			var viewBox = root.getAttribute("viewBox");
			root.removeAttribute("width");
			root.removeAttribute("height");
			root.style.maxWidth = "";
			answer({ type: "diagram", maxWidth: maxWidth, width: width, viewBox: viewBox, svg: new XMLSerializer().serializeToString(root) });
		}, function (error) {
			stage.textContent = "";
			failed(error);
		});
	};
};`;

// Writes the formula into the box at its size and measures it.
const MATH_DRAWER = `var setUp = function () {
	var box = document.getElementById("box");
	if (!window.katex) throw new Error("KaTeX didn't load.");
	draw = function (job, answer) {
		var markup = katex.renderToString(job.source, SETTINGS);
		box.style.fontSize = job.size + "px";
		box.innerHTML = markup;
		var shape = box.getBoundingClientRect();
		box.textContent = "";
		answer({ type: "math", markup: markup, width: Math.ceil(shape.width), height: Math.ceil(shape.height) });
	};
};`;

// JSON a script can hold: nothing in it closes the script.
const scriptJson = (value: unknown): string => JSON.stringify(value).replace(/</g, "\\u003c");

const LIBRARY: Record<DrawKind, string> = { diagram: "mermaid.js", math: "katex.js" };

/** The page that draws `kind`, with its library from `libraries` in it. */
export const rendererPage = (kind: DrawKind, libraries: ReadonlyMap<string, string>): string => {
	const settings =
		kind === "diagram"
			? mermaidSettings(diagramTheme(token, "dark"))
			: mathSettings(true, "mathml", color.destructive);
	const drawer = kind === "diagram" ? DIAGRAM_DRAWER : MATH_DRAWER;
	const html = [
		`<style>${STAGE_CSS}${MATH_CSS}</style>`,
		'<div id="stage"></div><div id="box" class="math"></div>',
		`<script src="${LIBRARY_PREFIX}${LIBRARY[kind]}"></script>`,
		`<script>(function () {\nvar SETTINGS = ${scriptJson(settings)};\n${drawer}\n${BRIDGE}\n})();</script>`,
	].join("");
	return inlineVisualDocument({ html, theme: "dark", variables: {} }, libraries).document;
};

/** What a drawing page said. */
export type Answer =
	| { readonly type: "ready" }
	| {
			readonly type: "diagram";
			readonly key: string;
			readonly maxWidth: string;
			readonly width: string | null;
			readonly viewBox: string | null;
			readonly svg: string;
	  }
	| {
			readonly type: "math";
			readonly key: string;
			readonly markup: string;
			readonly width: number;
			readonly height: number;
	  }
	| { readonly type: "empty"; readonly key: string }
	| { readonly type: "failed"; readonly key: string; readonly reason: string };

const isText = (value: unknown): value is string => typeof value === "string";
const isTextOrNull = (value: unknown) => value === null || isText(value);
const isSize = (value: unknown): value is number =>
	typeof value === "number" && Number.isFinite(value) && value >= 0;

/** What a drawing page said through the bridge, or null for anything else, such as its reports. */
export const readAnswer = (data: string): Answer | null => {
	const said = readBridgeReport(data);
	if (typeof said !== "object" || said === null) return null;
	const answer = said as Record<string, unknown>;
	if (answer.type === "ready") return { type: "ready" };
	if (!isText(answer.key)) return null;
	switch (answer.type) {
		case "diagram":
			return isText(answer.maxWidth) &&
				isTextOrNull(answer.width) &&
				isTextOrNull(answer.viewBox) &&
				isText(answer.svg)
				? (answer as Answer)
				: null;
		case "math":
			return isText(answer.markup) && isSize(answer.width) && isSize(answer.height)
				? (answer as Answer)
				: null;
		case "empty":
			return { type: "empty", key: answer.key };
		case "failed":
			return isText(answer.reason)
				? { type: "failed", key: answer.key, reason: answer.reason }
				: null;
		default:
			return null;
	}
};

/** The drawing a page's answer to `job` makes. */
export const drawingOf = (answer: Exclude<Answer, { type: "ready" }>, job: DrawJob): Drawing => {
	const noSize: Drawing = { state: "failed", reason: t("The drawing has no size.") };
	switch (answer.type) {
		case "diagram": {
			const shape = drawingSize(answer);
			if (!shape) return noSize;
			const { width, height } = shape;
			return {
				state: "drawn",
				body: `<img src="${svgUrl(answer.svg, width, height)}" alt="">`,
				width,
				height,
			};
		}
		case "math":
			if (answer.width === 0 || answer.height === 0) return noSize;
			return {
				state: "drawn",
				body: `<div class="math" style="font-size:${job.size}px">${answer.markup}</div>`,
				width: answer.width,
				height: answer.height,
			};
		case "empty":
			return { state: "failed", reason: t("The drawing came out empty.") };
		case "failed":
			return { state: "failed", reason: failureReason(answer.reason) };
	}
};

// Only the drawing's own picture and look. The page runs nothing, so a
// script that got past the cleaning couldn't run either.
const SHOWN_POLICY = "default-src 'none'; img-src data:; style-src 'unsafe-inline'";

const SHOWN_CSS =
	"html,body{margin:0;padding:0;background:transparent;overflow:hidden;-webkit-touch-callout:none;-webkit-user-select:none;user-select:none;-webkit-text-size-adjust:100%}img{display:block;width:100%;height:100%}";

/** The page that shows a drawing's body, filling its view. */
export const shownPage = (body: string): string =>
	`<!doctype html><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${SHOWN_POLICY}"><meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no"><style>${SHOWN_CSS}${MATH_CSS}</style>${body}`;
