// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The desktop system the app runs on, and the wording that follows from a
// system: macOS and iOS keep the text as written, the others take an
// i18next context, so "this Mac" reads "this PC" on Windows and "this
// computer" on Linux, and "iOS Settings" reads "Android settings" on
// Android.

/** A desktop system MaxHQ's app runs on. */
export type AppOs = "macos" | "windows" | "linux";

/** A system MaxHQ's interface runs on: the desktop app's, or the phone app's. */
export type InterfaceOs = AppOs | "ios" | "android";

/**
 * The system a web view's user agent names. WebView2 says "Windows", the
 * macOS web view says "Macintosh", and WebKitGTK says "X11; Linux" unless
 * a site quirk dresses it up, which an app page never gets. Anything else
 * reads as Linux, the one the other two leave.
 */
export const osOfUserAgent = (userAgent: string): AppOs =>
	userAgent.includes("Windows") ? "windows" : userAgent.includes("Mac") ? "macos" : "linux";

/** The i18next context a system's wording takes. */
export type OsWording = "windows" | "linux" | "android";

/**
 * The wording for a system: none for macOS and iOS, so their text stays as
 * written, and for a device whose system isn't known yet. `os` is an
 * interface system or a device's, as Rust names it ("macos", "windows",
 * "linux", "freebsd"), and every Unix other than macOS and Android reads as
 * Linux.
 */
export const wordingFor = (os: string | null | undefined): OsWording | undefined => {
	if (!os || os === "macos" || os === "ios") return undefined;
	return os === "windows" || os === "android" ? os : "linux";
};
