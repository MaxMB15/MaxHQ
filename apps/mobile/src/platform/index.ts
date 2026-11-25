// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What differs between the iOS and Android apps, in one place, so screens
// ask for a rule rather than check the system themselves. It imports no
// React Native, so the theme and the tests can use it. The hooks and calls
// that need React Native sit next to it in back.ts and save.ts.

import { OS, type PhoneOs } from "./os";

export { OS, type PhoneOs };

export type PlatformRules = {
	/** The monospace face: Menlo on iOS, as the desktop shows, and the system's on Android. */
	readonly monoFont: string;
	/**
	 * Whether a modal screen is a sheet that starts below the status bar.
	 * iOS's page sheets are, and Android's modals fill the screen edge to
	 * edge, under the status bar.
	 */
	readonly sheetsBelowStatusBar: boolean;
	/**
	 * Whether a saved file goes through the share sheet, where iOS has Save
	 * to Files. Android's share sheet takes no files, so it saves into a
	 * folder the person picks.
	 */
	readonly savesByShare: boolean;
	/**
	 * The i18next context for strings that name the system, such as "iOS
	 * Settings". The Android text sits under the key with `_android` after
	 * it, and no context keeps the iOS text.
	 */
	readonly context: "android" | undefined;
	/**
	 * The name a phone pairs under until the person types one. Android's
	 * own model names are often codes, such as "SM-S921B", so it's the
	 * system's name instead.
	 */
	readonly phoneName: string;
};

export const rulesFor = (os: PhoneOs): PlatformRules =>
	os === "ios"
		? {
				monoFont: "Menlo",
				sheetsBelowStatusBar: true,
				savesByShare: true,
				context: undefined,
				phoneName: "iPhone",
			}
		: {
				monoFont: "monospace",
				sheetsBelowStatusBar: false,
				savesByShare: false,
				context: "android",
				phoneName: "Android",
			};

/** The rules for the system this app runs on. */
export const platform: PlatformRules = rulesFor(OS);
