// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual as one document that needs nothing else, for a web view with no
// server behind it, as the phone's is. The libraries it has go into the
// page in place of the scripts that name them, and a content policy at the
// top keeps the page off the network. A small script ahead of the page
// reports, through the web view's bridge, what the policy blocked, the
// page's errors and the links the user clicked, in the frame's own words.

import type { Theme } from "../theme/theme";

import { LIBRARY_PATH, LIBRARY_PREFIX, visualDocument } from "./frame";

/**
 * What a self-contained visual may load: inline scripts and styles, which
 * carry its libraries, and what it makes itself. Nothing from the network.
 */
export const INLINE_VISUAL_POLICY = [
	"default-src 'none'",
	"script-src 'unsafe-inline' 'unsafe-eval' blob: data:",
	"style-src 'unsafe-inline' blob: data:",
	"img-src blob: data:",
	"font-src data:",
	"media-src blob: data:",
	"connect-src 'none'",
	"worker-src blob:",
	"frame-src 'none'",
	"form-action 'none'",
	"base-uri 'none'",
].join("; ");

// Says what happens in the page through the bridge a React Native web view
// gives it, as the desktop's frame says it with postMessage. A link within
// the page moves there, and any other goes to the app, only for a click the
// user made.
const REPORTER = `(function () {
	var tell = function (report) {
		if (window.ReactNativeWebView) window.ReactNativeWebView.postMessage(JSON.stringify(report));
	};
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
 * The document for a visual drawn with no server: the content policy and
 * the reporter first, then the page as the frame writes it, with each
 * script that loads one of `libraries` (by its file, such as `chart.js`)
 * carrying that library's text instead.
 */
export const inlineVisualDocument = (
	visual: {
		readonly html: string;
		readonly theme: Theme;
		readonly variables: Readonly<Record<string, string>>;
	},
	libraries: ReadonlyMap<string, string>
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
	const head = `<meta http-equiv="Content-Security-Policy" content="${INLINE_VISUAL_POLICY}"><script>${REPORTER}</script>`;
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
