// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual as one document that needs nothing else, for a web view with no
// server behind it, as the phone's is. The libraries it has go into the
// page in place of the scripts that name them, and a content policy at the
// top keeps the page off the network, but for the hosts Load this time let
// through. A small script ahead of the page reports, through the web view's
// bridge, how tall the page is, what the policy blocked, the page's errors
// and the links the user clicked, in the frame's own words.

import type { Theme } from "../theme/theme";

import { isPlainHost, LIBRARY_PATH, LIBRARY_PREFIX, MOST_HOSTS, visualDocument } from "./frame";

/**
 * What a self-contained visual may load: inline scripts and styles, which
 * carry its libraries, and what it makes itself. From the network, only
 * code, styles, fonts and pictures from `hosts` over https, each a plain
 * host and only the first few, as the desktop's frame. It never connects,
 * frames or posts a form.
 */
export const inlineVisualPolicy = (hosts: readonly string[]): string => {
	const extra = hosts
		.filter(isPlainHost)
		.slice(0, MOST_HOSTS)
		.map((host) => ` https://${host}`)
		.join("");
	return [
		"default-src 'none'",
		`script-src 'unsafe-inline' 'unsafe-eval' blob: data:${extra}`,
		`style-src 'unsafe-inline' blob: data:${extra}`,
		`img-src blob: data:${extra}`,
		`font-src data:${extra}`,
		"media-src blob: data:",
		"connect-src 'none'",
		"worker-src blob:",
		"frame-src 'none'",
		"form-action 'none'",
		"base-uri 'none'",
	].join("; ");
};

/** The policy with no hosts let through: nothing from the network. */
export const INLINE_VISUAL_POLICY = inlineVisualPolicy([]);

// Says what happens in the page through the bridge a React Native web view
// gives it, as the desktop's frame says it with postMessage. The height is
// the content's unless the page scrolls, as one whose root is 100% tall
// does, and goes again each time it changes. A link within the page moves
// there, and any other goes to the app, only for a click the user made.
const REPORTER = `(function () {
	var tell = function (report) {
		if (window.ReactNativeWebView) window.ReactNativeWebView.postMessage(JSON.stringify(report));
	};
	var last = -1;
	var measure = function () {
		var root = document.documentElement;
		var height = Math.ceil(root.scrollHeight > root.clientHeight ? root.scrollHeight : root.getBoundingClientRect().height);
		if (height === last) return;
		last = height;
		tell({ type: "maxhq-visual:height", height: height });
	};
	addEventListener("DOMContentLoaded", function () {
		if (typeof ResizeObserver === "function") {
			var observer = new ResizeObserver(measure);
			observer.observe(document.documentElement);
			if (document.body) observer.observe(document.body);
		}
		measure();
	});
	addEventListener("load", measure);
	addEventListener("securitypolicyviolation", function (event) {
		tell({ type: "maxhq-visual:blocked", url: event.blockedURI, what: event.effectiveDirective });
	});
	addEventListener("error", function (event) {
		if (event instanceof ErrorEvent)
			tell({ type: "maxhq-visual:error", message: event.message || String(event.error), line: event.lineno || null });
	}, true);
	addEventListener("unhandledrejection", function (event) {
		var reason = event.reason;
		tell({ type: "maxhq-visual:error", message: reason instanceof Error ? reason.message : String(reason), line: null });
	});
	addEventListener("click", function (event) {
		if (!event.isTrusted || event.defaultPrevented) return;
		var link = event.target && event.target.closest ? event.target.closest("a[href]") : null;
		if (!link) return;
		var href = link.getAttribute("href") || "";
		if (href.charAt(0) === "#") return;
		event.preventDefault();
		try { tell({ type: "maxhq-visual:link", url: new URL(href, location.href).href }); } catch (error) {}
	});
})();`;

// The page lays out at the width of the view it's in, in the pixels the
// view measures. Without it, a phone's web view lays the page out at a
// desktop's width and shrinks it.
const VIEWPORT = '<meta name="viewport" content="width=device-width, initial-scale=1">';

// A script tag that loads a library the frame would serve, with whatever
// else it says around the address.
const LIBRARY_SCRIPT = new RegExp(
	`<script\\b([^>]*?)\\s+src=(["'])${LIBRARY_PATH}([^"'?#]+)\\2([^>]*)>\\s*</script\\s*>`,
	"gi"
);

// Any other mention of a library's address, such as a module's import.
const LIBRARY_MENTION = new RegExp(`${LIBRARY_PATH}([A-Za-z0-9._/-]+)`, "g");

// A library's text can't end the script it sits in.
const scriptText = (text: string): string => text.replace(/<\/(script)/gi, "<\\/$1");

/** A visual's document with its libraries in it, and the ones it named that aren't there. */
export type InlineVisual = {
	readonly document: string;
	/** Each library the page asked for that wasn't given, as `maxhq:lib/three.js`. */
	readonly missing: readonly string[];
};

/**
 * The document for a visual drawn with no server: the content policy, with
 * `hosts` let through, and the reporter first, then the page as the frame
 * writes it, with each script that loads one of `libraries` (by its file,
 * such as `chart.js`) carrying that library's text instead.
 */
export const inlineVisualDocument = (
	visual: {
		readonly html: string;
		readonly theme: Theme;
		readonly variables: Readonly<Record<string, string>>;
	},
	libraries: ReadonlyMap<string, string>,
	hosts: readonly string[] = []
): InlineVisual => {
	const written = visualDocument(visual).replace(/^<!doctype html>/, "");
	const left = written.replace(LIBRARY_SCRIPT, (tag, _before, _quote, name: string) =>
		libraries.has(name) ? "" : tag
	);
	const missing = [
		...new Set([...left.matchAll(LIBRARY_MENTION)].map(([, name]) => LIBRARY_PREFIX + name)),
	];
	const page = written.replace(
		LIBRARY_SCRIPT,
		(tag, before: string, _quote, name: string, after: string) => {
			const text = libraries.get(name);
			return text === undefined
				? tag
				: `<script${before}${after}>${scriptText(text)}</script>`;
		}
	);
	const head = `<meta http-equiv="Content-Security-Policy" content="${inlineVisualPolicy(hosts)}">${VIEWPORT}<script>${REPORTER}</script>`;
	return { document: `<!doctype html>${head}${page}`, missing };
};

// A web address with a host, or a mail one. The page wrote it out whole, and
// React Native's URL can't say a link's scheme.
const LINK = /^(?:https?:\/\/[^/?#\s]+|mailto:\S)/i;

/** Whether a link a visual's page reports opens outside the app: a web or mail address. */
export const opensFromVisual = (url: string): boolean => LINK.test(url);

/** What the bridge carried from a visual's page, or null for anything else. */
export const readBridgeReport = (data: string): unknown => {
	try {
		return JSON.parse(data) as unknown;
	} catch {
		return null;
	}
};
