// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What differs between the iOS and Android apps, in one place, so screens
// ask for a rule rather than check the system themselves. It imports no
// React Native, so the theme and the tests can use it. The hooks and calls
// that need React Native sit next to it in back.ts and save.ts.

import type { PushService } from "@maxhq/shared/bindings";

import { OS, type PhoneOs } from "./os";

export { OS, type PhoneOs };

/** Where the phone gets a newer build of itself. */
export type UpdateChannel = "testflight" | "apk";

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
	 * Whether the file viewer shows a PDF. iOS's web view draws one, and
	 * Android's can't, so it says so and offers Save.
	 */
	readonly opensPdf: boolean;
	/**
	 * Where a newer build comes from: TestFlight on iOS, which goes by the
	 * version, and on Android the APK on GitHub Releases, which installs
	 * over the app only with a higher version code.
	 */
	readonly updatesThrough: UpdateChannel;
	/**
	 * The name a phone pairs under until the person types one. Android's
	 * own model names are often codes, such as "SM-S921B", so it's the
	 * system's name instead.
	 */
	readonly phoneName: string;
	/** The service the phone's pushes come through: APNs on iOS, FCM on Android. */
	readonly pushService: PushService;
	/**
	 * Whether Deny on a notification answers with the app left in the
	 * background. iOS wakes it there for the answer. Android starts the
	 * app's screen for it, which the lock screen unlocks for first.
	 */
	readonly answersInBackground: boolean;
	/** Whether notifications sit in channels the app makes, one per push kind, as on Android. */
	readonly notificationChannels: boolean;
};

export const rulesFor = (os: PhoneOs): PlatformRules =>
	os === "ios"
		? {
				monoFont: "Menlo",
				sheetsBelowStatusBar: true,
				savesByShare: true,
				opensPdf: true,
				updatesThrough: "testflight",
				phoneName: "iPhone",
				pushService: "apns",
				answersInBackground: true,
				notificationChannels: false,
			}
		: {
				monoFont: "monospace",
				sheetsBelowStatusBar: false,
				savesByShare: false,
				opensPdf: false,
				updatesThrough: "apk",
				phoneName: "Android",
				pushService: "fcm",
				answersInBackground: false,
				notificationChannels: true,
			};

/** The rules for the system this app runs on. */
export const platform: PlatformRules = rulesFor(OS);
