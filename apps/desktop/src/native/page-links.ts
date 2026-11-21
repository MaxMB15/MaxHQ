// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Links that leave the app and open in the browser. The app keeps the
// window and its frames on its own pages, and tells the page about each web
// or mail address one tried to go to. A link you just clicked in the app
// opens at once, as it always did. Anything else, such as a visual's frame
// sent to a web page by its script, waits for Open in the window's
// question, which shows the whole address. A visual's links ask the same
// way, and all of them share one gate, so one question shows at a time.

import { t } from "@maxhq/shared/i18n";
import { type LinkQuestion, linkGate, pageLinkQuestion } from "@maxhq/shared/visuals/links";

import type { Services } from "@/app/services";
import { asking, requestConfirm } from "@/components/confirm";
import { events } from "@/data/tauri-commands";

import { inApp } from "./platform";

const gate = linkGate();

/** Opens `url` in the browser, which the app does for a new window. */
const openInBrowser = (url: string) => {
	window.open(url, "_blank", "noreferrer");
};

/**
 * Asks `question` over the whole of `url`, and opens it in the browser on
 * Open. Nothing is asked while another question waits, which it would
 * otherwise close.
 */
export const askToOpen = async (
	services: Services,
	url: string,
	question: LinkQuestion
): Promise<void> => {
	if (asking(services) || !gate.ask(url, Date.now())) return;
	try {
		if (await requestConfirm(services, { ...question, link: url, action: t("Open") }))
			openInBrowser(url);
	} finally {
		gate.answered(Date.now());
	}
};

/** How long after a click on a link the app's word about it still matches. */
export const CLICK_MS = 2000;

const sameUrl = (url: string): string => {
	try {
		return new URL(url).href;
	} catch {
		return url;
	}
};

/** The app's own link you clicked last, which the app's word can match once. */
export type LinkClicks = {
	readonly note: (href: string, at: number) => void;
	/** Whether `url` is the link clicked last, under `CLICK_MS` ago. A match is used up. */
	readonly take: (url: string, now: number) => boolean;
};

export const linkClicks = (): LinkClicks => {
	let last: { readonly href: string; readonly at: number } | null = null;
	return {
		note: (href, at) => {
			last = { href: sameUrl(href), at };
		},
		take: (url, now) => {
			if (!last || now - last.at > CLICK_MS || last.href !== sameUrl(url)) return false;
			last = null;
			return true;
		},
	};
};

/** Opens an address the app kept the window from: at once after a click on it, else after asking. */
export const openRefused = (services: Services, clicks: LinkClicks, url: string): void => {
	if (clicks.take(url, Date.now())) openInBrowser(url);
	else void askToOpen(services, url, pageLinkQuestion());
};

/**
 * Notes the links you click and opens what the app kept the window from,
 * for as long as the window runs. Returns a function that stops. In a
 * browser the page goes where its links say, so nothing is watched.
 */
export const watchPageLinks = (services: Services): (() => void) => {
	if (!inApp()) return () => undefined;
	const clicks = linkClicks();
	const onClick = (event: MouseEvent) => {
		const link = (event.target as Element | null)?.closest?.("a[href]");
		if (event.isTrusted && link instanceof HTMLAnchorElement)
			clicks.note(link.href, Date.now());
	};
	window.addEventListener("click", onClick, true);
	window.addEventListener("auxclick", onClick, true);
	let stopped = false;
	let stop: (() => void) | undefined;
	events.openAsked
		.listen((event) => openRefused(services, clicks, event.payload.url))
		.then((unlisten) => {
			// Stopping is async, and a window going away may have lost its
			// events already.
			stop = () =>
				void Promise.resolve()
					.then(unlisten)
					.catch(() => undefined);
			if (stopped) stop();
		}, console.error);
	return () => {
		stopped = true;
		stop?.();
		window.removeEventListener("click", onClick, true);
		window.removeEventListener("auxclick", onClick, true);
	};
};
