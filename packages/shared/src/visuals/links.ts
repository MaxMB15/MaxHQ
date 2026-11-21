// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the app asks before it opens a link from a visual, on the desktop
// and the phone. A page can say it was clicked when it wasn't, and
// whatever a link's address carries leaves the app with it, so each link
// waits for Open in a question that shows the whole address. Only web and
// mail links get that far, one question at a time, and the next comes a
// second after the last was answered at the soonest.

import { t } from "../i18n";

import { opensFromVisual } from "./inline";

/** How long after one link's question the next can come, in milliseconds. */
export const LINK_GAP_MS = 1000;

/** Which links a window asks about. One gate serves a whole window. */
export type LinkGate = {
	/**
	 * Whether to ask about `url` now: a web or mail address, with no other
	 * question open and the last answered a second ago or more. Once it says
	 * yes, it says no until `answered`.
	 */
	readonly ask: (url: string, now: number) => boolean;
	/** The question it let through was answered, either way. */
	readonly answered: (now: number) => void;
};

/** A gate with nothing asked yet. */
export const linkGate = (): LinkGate => {
	let asking = false;
	let last = -Infinity;
	return {
		ask: (url, now) => {
			if (asking || !opensFromVisual(url) || now - last < LINK_GAP_MS) return false;
			asking = true;
			return true;
		},
		answered: (now) => {
			asking = false;
			last = now;
		},
	};
};

/** The question before a link opens, which the window shows with the whole address. */
export type LinkQuestion = { readonly title: string; readonly detail: string };

/**
 * The question for a link a visual gave. `from` names the agent that
 * showed it, when the visual came from one.
 */
export const visualLinkQuestion = (from: string | null): LinkQuestion => ({
	title: t("Open this link?"),
	detail:
		from === null
			? t(
					"This visual wants to open this address in your browser. Anything in the address leaves MaxHQ, so open it only if you expect it."
				)
			: t(
					"The visual from {{from}} wants to open this address in your browser. Anything in the address leaves MaxHQ, so open it only if you expect it.",
					{ from }
				),
});

/**
 * The question for a link something in the window went to without a click
 * the window saw, such as a link dropped on it.
 */
export const pageLinkQuestion = (): LinkQuestion => ({
	title: t("Open this link?"),
	detail: t(
		"Something in this window tried to go to this address, and MaxHQ stayed where it was. Anything in the address leaves MaxHQ, so open it in your browser only if you expect it."
	),
});
