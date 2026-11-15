// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How the boxes you type prompts and notes in check spelling, and whether
// the system may correct it. Nothing corrects, capitalizes or curls quotes
// by default. The same settings say what Enter does in the composer while
// an agent works.
//
// A webview's own spell check also brings the system's automatic
// corrections, smart quotes and dashes and text replacements with it,
// whatever a field asks. So a field gets it only when you turn on
// Correct spelling automatically, or where there's no system checker, as
// in a browser, which doesn't correct. Otherwise a rich editor underlines
// through the system checker, and a plain field doesn't check.

import { type BusySend, busySendOf, DEFAULT_BUSY_SEND } from "../composer/send";

/** The typing settings, kept per Mac. */
export type TypingPrefs = {
	/** Whether misspelled words get underlined. */
	readonly checkSpelling: boolean;
	/** Whether the system corrects spelling as you type. Needs `checkSpelling`. */
	readonly correctSpelling: boolean;
	/** What Enter does while an agent works. */
	readonly busySend: BusySend;
};

export const DEFAULT_TYPING: TypingPrefs = {
	checkSpelling: true,
	correctSpelling: false,
	busySend: DEFAULT_BUSY_SEND,
};

// Bump when the saved shape changes, so old settings read as the defaults.
const VERSION = 1;

/** Saves the settings as text. */
export const writeTypingPrefs = (prefs: TypingPrefs): string =>
	JSON.stringify({ version: VERSION, ...prefs });

/** Reads saved settings, defaulting each part that doesn't check out. */
export const readTypingPrefs = (raw: string | null): TypingPrefs => {
	if (raw === null) return DEFAULT_TYPING;
	let saved: unknown;
	try {
		saved = JSON.parse(raw);
	} catch {
		return DEFAULT_TYPING;
	}
	if (typeof saved !== "object" || saved === null) return DEFAULT_TYPING;
	const { version, checkSpelling, correctSpelling, busySend } = saved as Record<string, unknown>;
	if (version !== VERSION) return DEFAULT_TYPING;
	return {
		checkSpelling:
			typeof checkSpelling === "boolean" ? checkSpelling : DEFAULT_TYPING.checkSpelling,
		correctSpelling:
			typeof correctSpelling === "boolean" ? correctSpelling : DEFAULT_TYPING.correctSpelling,
		busySend: busySendOf(busySend),
	};
};

/**
 * Who checks a field's spelling: the system checker through the app, the
 * webview's own, or nobody.
 */
export type SpellingMode = "native" | "webview" | "off";

/** Whether the system corrects as you type, which only runs with spell check on. */
export const corrects = (prefs: TypingPrefs): boolean =>
	prefs.checkSpelling && prefs.correctSpelling;

/**
 * Who checks spelling in a field. `nativeSpelling` says the system checker
 * is there, and `rich` that the field can draw its underlines, as the
 * prompt and note editors can.
 */
export const spellingMode = (
	prefs: TypingPrefs,
	{ nativeSpelling, rich }: { readonly nativeSpelling: boolean; readonly rich: boolean }
): SpellingMode => {
	if (corrects(prefs)) return "webview";
	if (!prefs.checkSpelling) return "off";
	if (!nativeSpelling) return "webview";
	return rich ? "native" : "off";
};

/** The attributes an editable element takes, as HTML writes them. */
export type TypingAttributes = {
	readonly spellcheck: "true" | "false";
	readonly autocorrect: "on" | "off";
	readonly autocapitalize: "off";
	readonly writingsuggestions: "false";
};

/** A field's typing attributes for a mode. */
export const typingAttributes = (prefs: TypingPrefs, mode: SpellingMode): TypingAttributes => ({
	spellcheck: mode === "webview" ? "true" : "false",
	autocorrect: corrects(prefs) ? "on" : "off",
	autocapitalize: "off",
	writingsuggestions: "false",
});
