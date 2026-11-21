// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual's frame page. It runs in a sandboxed frame on an opaque origin
// with no network, waits for a page from the app, and writes it as the
// frame's document. Writing a document takes away the listeners the window
// had, so this script listens again before it writes, and its listeners see
// the page from its first line: how tall it is, its errors, what the content
// policy blocked, the files it asked for that aren't there, and the links
// the user clicked.

import {
	type FrameReport,
	LIBRARY_PATH,
	LIBRARY_PREFIX,
	readShowVisual,
	visualDocument,
} from "@maxhq/shared/visuals/frame";

const tell = (report: FrameReport) => window.parent.postMessage(report, "*");

const LINKS = new Set(["http:", "https:", "mailto:"]);

const urlOf = (written: string): URL | null => {
	try {
		return new URL(written, document.baseURI);
	} catch {
		return null;
	}
};

// A file of this frame's own, such as `data.json` or a library, by the name
// the page gave it.
const ownPath = (url: URL | null): string | null => {
	if (!url || url.protocol !== location.protocol || url.host !== location.host) return null;
	return url.pathname.startsWith(LIBRARY_PATH)
		? LIBRARY_PREFIX + url.pathname.slice(LIBRARY_PATH.length)
		: decodeURIComponent(url.pathname.replace(/^\//, ""));
};

const messageOf = (reason: unknown): string =>
	reason instanceof Error ? reason.message : String(reason);

const listen = () => {
	// A script's error comes as an ErrorEvent. A file that didn't load fires
	// a plain error at its element, which only capturing sees.
	window.addEventListener(
		"error",
		(event: Event) => {
			if (event instanceof ErrorEvent) {
				tell({
					type: "maxhq-visual:error",
					message: event.message || messageOf(event.error),
					line: event.lineno || null,
				});
				return;
			}
			const element = event.target as Element | null;
			const written = element?.getAttribute?.("src") ?? element?.getAttribute?.("href");
			const path = written ? ownPath(urlOf(written)) : null;
			if (path) tell({ type: "maxhq-visual:missing", path });
		},
		true
	);
	window.addEventListener("unhandledrejection", (event) =>
		tell({ type: "maxhq-visual:error", message: messageOf(event.reason), line: null })
	);
	window.addEventListener("securitypolicyviolation", (event) => {
		// Asking this frame for a file, as fetch("data.json") does, is asking
		// for one that isn't there.
		const path = ownPath(urlOf(event.blockedURI));
		if (path) tell({ type: "maxhq-visual:missing", path });
		else
			tell({
				type: "maxhq-visual:blocked",
				url: event.blockedURI,
				what: event.effectiveDirective,
			});
	});
	// After the page's own handlers, so a link it handles stays its own. A
	// link within the page moves there. Any other goes to the app, which
	// opens it in the browser, and only for a click the user made.
	window.addEventListener("click", (event) => {
		if (!event.isTrusted || event.defaultPrevented) return;
		const link = (event.target as Element | null)?.closest?.("a[href]");
		const url = urlOf(link?.getAttribute("href") ?? "");
		if (!link || !url) return;
		if (url.hash && url.href.split("#")[0] === location.href.split("#")[0]) return;
		event.preventDefault();
		if (LINKS.has(url.protocol)) tell({ type: "maxhq-visual:link", url: url.href });
	});
};

// The page's height, which is its content's unless it scrolls, as one whose
// root is 100% tall does.
const watchHeight = () => {
	const root = document.documentElement;
	let last = -1;
	const report = () => {
		const height = Math.ceil(
			root.scrollHeight > root.clientHeight
				? root.scrollHeight
				: root.getBoundingClientRect().height
		);
		if (height === last) return;
		last = height;
		tell({ type: "maxhq-visual:height", height });
	};
	const observer = new ResizeObserver(report);
	observer.observe(root);
	if (document.body) observer.observe(document.body);
	report();
};

const onShow = (event: MessageEvent) => {
	if (event.source !== window.parent) return;
	const show = readShowVisual(event.data);
	if (!show) return;
	document.open();
	listen();
	document.write(visualDocument(show));
	document.close();
	watchHeight();
};

// Writing the page takes this listener away too, so a frame shows one page.
window.addEventListener("message", onShow);
tell({ type: "maxhq-visual:ready" });
