// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Spell check that follows macOS: its languages, its learned words and
// Ignore Spelling. The composer underlines what `checkSpelling` finds and
// opens `spellingMenu` on a right click.

import { t } from "@maxhq/shared/i18n";
import type { Misspelling, SpellingChoice } from "@maxhq/shared/bindings";
import { commands } from "@/data/tauri-commands";

import { unwrap } from "./failure";
import { inApp, onMac } from "./platform";

/**
 * Whether spell check comes from macOS. Where it doesn't, `checkSpelling`
 * finds nothing, so turn on the text area's own `spellcheck` instead.
 */
export const hasNativeSpelling = (): boolean => inApp() && onMac();

/**
 * The misspelled words in `text`. Offsets count UTF-16 code units, the
 * same as `String.prototype.slice`. Each sentence gets its own language,
 * so English and Dutch can share a message. A 2,000 word message takes
 * about 40 ms, so check after a pause in typing rather than on every key.
 * Empty outside the app.
 */
export const checkSpelling = (text: string): Promise<Misspelling[]> =>
	inApp() ? unwrap(commands.checkSpelling(text)) : Promise.resolve([]);

/**
 * Shows the native spelling menu at a point in the webview, such as a
 * `contextmenu` event's `clientX` and `clientY`. Pass the misspelling under
 * the pointer for guesses, Ignore Spelling and Learn Spelling, or null for
 * a menu with only Cut, Copy and Paste. Resolves once the menu closes.
 * After `ignored` or `learned`, check the text again.
 */
export const spellingMenu = (
	misspelling: Misspelling | null,
	at: { readonly x: number; readonly y: number }
): Promise<SpellingChoice> =>
	inApp()
		? unwrap(
				commands.spellingMenu(
					misspelling?.word ?? null,
					misspelling?.language ?? null,
					{
						x: at.x,
						y: at.y,
					},
					{
						no_guesses: t("No Guesses Found"),
						ignore: t("Ignore Spelling"),
						learn: t("Learn Spelling"),
						cut: t("Cut"),
						copy: t("Copy"),
						paste: t("Paste"),
					}
				)
			)
		: Promise.resolve({ kind: "none" });

export type { Misspelling, SpellingChoice };
