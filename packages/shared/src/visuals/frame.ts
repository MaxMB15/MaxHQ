// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a visual's frame and the app say to each other, and what either side
// works out from it. The frame runs a page nobody checked, on an opaque
// origin with no network, so everything it sends is read as untrusted.
//
// The app sends the page with the theme's colours. The frame writes the
// document, then reports how tall it is, its script errors, what its content
// policy blocked, the files it asked for that aren't there, and links the
// user clicked.

import type { Theme } from "../theme/theme";

import { MAX_VISUAL_HEIGHT } from "./visual";

/** How a page names a library the app serves, as in `maxhq:lib/chart.js`. */
export const LIBRARY_PREFIX = "maxhq:lib/";

/** Where the frame serves those libraries. */
export const LIBRARY_PATH = "/visual-lib/";

/** The theme's CSS variables a page can use. */
export const THEME_VARIABLES = [
	"--background",
	"--foreground",
	"--muted",
	"--muted-foreground",
	"--border",
	"--accent",
	"--primary",
	"--chart-1",
	"--chart-2",
	"--chart-3",
	"--chart-4",
	"--chart-5",
	"--chart-6",
	"--font-sans",
	"--font-mono",
	"--radius",
] as const;

/** The page and the theme it draws in, from the app. */
export type ShowVisual = {
	readonly type: "maxhq-visual:show";
	readonly html: string;
	readonly theme: Theme;
	/** The theme's variables by name, `--chart-1` and the rest. */
	readonly variables: Readonly<Record<string, string>>;
};

/** What the frame tells the app. */
export type FrameReport =
	/** The frame is loaded and waits for its page. */
	| { readonly type: "maxhq-visual:ready" }
	/** How tall the page is now, in CSS pixels. */
	| { readonly type: "maxhq-visual:height"; readonly height: number }
	/** A script threw, or a promise failed with nobody to catch it. */
	| {
			readonly type: "maxhq-visual:error";
			readonly message: string;
			readonly line: number | null;
	  }
	/** The content policy refused a load: its URL and the kind of load, such as `script-src`. */
	| { readonly type: "maxhq-visual:blocked"; readonly url: string; readonly what: string }
	/** A file the page names next to itself didn't load, as the page wrote it. */
	| { readonly type: "maxhq-visual:missing"; readonly path: string }
	/** The user clicked a link to this URL. */
	| { readonly type: "maxhq-visual:link"; readonly url: string };

const MOST_TEXT = 500;

const isText = (value: unknown): value is string =>
	typeof value === "string" && value.length <= MOST_TEXT * 4;

/** A report from a frame's message data, or null for anything else. */
export const readFrameReport = (data: unknown): FrameReport | null => {
	if (typeof data !== "object" || data === null) return null;
	const message = data as Record<string, unknown>;
	switch (message.type) {
		case "maxhq-visual:ready":
			return { type: message.type };
		case "maxhq-visual:height":
			return typeof message.height === "number" && Number.isFinite(message.height)
				? { type: message.type, height: message.height }
				: null;
		case "maxhq-visual:error":
			return isText(message.message)
				? {
						type: message.type,
						message: message.message.slice(0, MOST_TEXT),
						line:
							typeof message.line === "number" && Number.isInteger(message.line)
								? message.line
								: null,
					}
				: null;
		case "maxhq-visual:blocked":
			return isText(message.url) && isText(message.what)
				? { type: message.type, url: message.url, what: message.what }
				: null;
		case "maxhq-visual:missing":
			return isText(message.path) ? { type: message.type, path: message.path } : null;
		case "maxhq-visual:link":
			return isText(message.url) ? { type: message.type, url: message.url } : null;
		default:
			return null;
	}
};

/** The page to show from the app's message data, or null for anything else. */
export const readShowVisual = (data: unknown): ShowVisual | null => {
	if (typeof data !== "object" || data === null) return null;
	const message = data as Partial<ShowVisual>;
	if (
		message.type !== "maxhq-visual:show" ||
		typeof message.html !== "string" ||
		(message.theme !== "light" && message.theme !== "dark") ||
		typeof message.variables !== "object" ||
		message.variables === null
	)
		return null;
	return message as ShowVisual;
};

// A value can't close the rule or the style element it sits in.
const cssValue = (value: string): string => value.replace(/[;{}<>]/g, "").trim();

// The page's own rules win over these, which only set where it starts.
const BASE_CSS = [
	":where(html){background:transparent;color:var(--foreground);font-family:var(--font-sans);font-size:14px;line-height:1.5;padding:8px}",
	":where(body){margin:0}",
].join("");

/**
 * Takes away the page's peer connections before its own scripts run. The
 * content policy has no word for them, and one reaches the network on its
 * own. It names `window`, so a test can run it on a stand-in.
 */
export const NO_PEERS =
	'["RTCPeerConnection","webkitRTCPeerConnection","RTCDataChannel"].forEach(function(name){try{delete window[name]}catch(error){}});';

// Where the tag that starts at `from`, just past its name, ends: past its
// `>` outside a quoted value, or at the end of the page for one left open.
const tagEnd = (html: string, from: number): number => {
	let quote = "";
	let valueNext = false;
	for (let at = from; at < html.length; at++) {
		const char = html.charAt(at);
		if (quote) {
			if (char === quote) quote = "";
		} else if (char === ">") return at + 1;
		else if (char === "=") valueNext = true;
		else if (valueNext && (char === '"' || char === "'")) quote = char;
		else if (!/\s/.test(char)) valueNext = false;
	}
	return html.length;
};

// A link that warms up a connection ahead of any load, which no content
// policy looks at, or one that spells its words in character references.
const WARMS = /prefetch|preconnect|prerender|&#/i;

/** The page without the link tags that reach out ahead of a load, such as `rel="preconnect"`. */
export const withoutWarmUps = (html: string): string => {
	const lower = html.toLowerCase();
	let kept = "";
	let from = 0;
	for (let at = lower.indexOf("<link", from); at !== -1; at = lower.indexOf("<link", from)) {
		const name = at + "<link".length;
		// Another tag whose name starts the same, such as `<linkbox>`.
		if (name < html.length && !/[\s/>]/.test(html.charAt(name))) {
			kept += html.slice(from, name);
			from = name;
			continue;
		}
		const end = tagEnd(html, name);
		const tag = html.slice(at, end);
		kept += html.slice(from, at) + (WARMS.test(tag) ? "" : tag);
		from = end;
	}
	return kept + html.slice(from);
};

/**
 * The document a frame writes for a page: what keeps it off the network
 * where the content policy can't, the theme's colours and the base styles
 * first in its head, then the page with its doctype and warm-up links taken
 * off, and each `maxhq:lib/` turned into the path the frame serves
 * libraries from.
 */
export const visualDocument = ({ html, theme, variables }: Omit<ShowVisual, "type">): string => {
	const declared = THEME_VARIABLES.filter((name) => typeof variables[name] === "string")
		.map((name) => `${name}:${cssValue(variables[name] ?? "")}`)
		.join(";");
	const page = withoutWarmUps(html.replace(/^\uFEFF?\s*<!doctype[^>]*>/i, "")).replaceAll(
		LIBRARY_PREFIX,
		LIBRARY_PATH
	);
	return `<!doctype html><meta http-equiv="x-dns-prefetch-control" content="off"><script>${NO_PEERS}</script><style>:root{color-scheme:${theme};${declared}}${BASE_CSS}</style>${page}`;
};

/** How the frame's height follows its page. */
export type HeightTrack = {
	/** The height the frame shows, in CSS pixels. */
	readonly height: number;
	/** How much the last report grew on the one before it. */
	readonly step: number;
	/** How many reports in a row grew by that step. */
	readonly repeats: number;
	/** The height before those reports started growing. */
	readonly before: number;
	/** Set once the page grew the same step too many times, which stops the following. */
	readonly stopped: boolean;
};

export const HEIGHT_START: HeightTrack = {
	height: 0,
	step: 0,
	repeats: 0,
	before: 0,
	stopped: false,
};

// A page whose height comes from the frame's, through `vh` or
// `height: 100%` with something added, grows by the same step each time the
// frame grows to fit it. This many such steps in a row is that loop rather
// than the page.
const LOOP_STEPS = 8;

/**
 * Where the height goes for a page that reports `reported`. It's capped at
 * the most a visual draws, and a page caught growing in a loop keeps the
 * height it had before the loop and isn't followed again until it reloads.
 */
export const nextHeight = (track: HeightTrack, reported: number): HeightTrack => {
	if (track.stopped) return track;
	const height = Math.min(MAX_VISUAL_HEIGHT, Math.max(0, Math.ceil(reported)));
	const step = height - track.height;
	if (step <= 0) return { ...track, height, step: 0, repeats: 0 };
	const repeats = step === track.step ? track.repeats + 1 : 1;
	const before = repeats === 1 ? track.height : track.before;
	if (repeats >= LOOP_STEPS) return { height: before, step, repeats, before, stopped: true };
	return { height, step, repeats, before, stopped: false };
};

/** The most hosts one visual can load from, after Load this time. */
export const MOST_HOSTS = 4;

const LABEL = /^(?!-)[a-z0-9-]{1,63}(?<!-)$/;

/**
 * Whether `host` is a plain web host name such as `cdn.example.com`, with an
 * optional port: lower case, two labels or more, the last starting with a
 * letter, so no address, no `localhost` and nothing that could add to a
 * content policy. The app's scheme takes the same list.
 */
export const isPlainHost = (host: string): boolean => {
	const [name = "", port, ...more] = host.split(":");
	if (more.length > 0 || name.length > 253) return false;
	if (port !== undefined && !/^[1-9][0-9]{0,4}$/.test(port)) return false;
	if (port !== undefined && Number(port) > 65535) return false;
	const labels = name.split(".");
	return (
		labels.length >= 2 &&
		labels.every((label) => LABEL.test(label)) &&
		/^[a-z]/.test(labels.at(-1) ?? "")
	);
};

/** What a blocked load means for the user. */
export type Blocked = {
	/** The host it came from, or the whole URL when it has none. */
	readonly host: string;
	/** Set when Load this time can let it through: an https host the page loads code or styles from. */
	readonly loadable: boolean;
};

/** The host behind a blocked load, and whether loading it this time could help. */
export const blockedLoad = (url: string, what: string): Blocked | null => {
	if (url === "inline" || url === "eval" || url === "wasm-eval") return null;
	// The scheme, then the host and port, past any user name.
	const parts = /^([a-z][a-z0-9+.-]*):\/\/(?:[^/?#@]*@)?([^/?#]+)/i.exec(url);
	// Not a URL with a host, so it shows as written.
	if (!parts) return { host: url.slice(0, 80), loadable: false };
	const https = parts[1]?.toLowerCase() === "https";
	const host = (parts[2] ?? "").toLowerCase().replace(https ? /:443$/ : /:80$/, "");
	return {
		host,
		loadable:
			https &&
			isPlainHost(host) &&
			!what.startsWith("connect-src") &&
			!what.startsWith("frame-src"),
	};
};

/**
 * The frame page's query: the theme, so it starts in the right colours, and
 * the hosts Load this time let through, which the app's scheme reads into
 * the page's content policy.
 */
export const visualFrameQuery = (theme: Theme, hosts: readonly string[]): string => {
	const allowed = hosts.filter(isPlainHost).slice(0, MOST_HOSTS);
	return `?theme=${theme}${allowed.length ? `&hosts=${allowed.join(",")}` : ""}`;
};
