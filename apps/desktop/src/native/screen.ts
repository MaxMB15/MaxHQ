// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The header's sun, which keeps this Mac's screen on. The app holds it, so
// it ends when the app quits, and saves it, so it's back on when the app
// opens again. In a browser, or on a system that can't keep its screen on,
// there's no sun.

import { commands } from "@/data/tauri-commands";

import { unwrap } from "./failure";
import { inApp } from "./platform";

/** Whether the sun is on, or null when there's none to show. */
export const screenOn = (): Promise<boolean | null> =>
	inApp() ? commands.screenOn() : Promise.resolve(null);

/** Turns the sun on or off. */
export const setScreenOn = (on: boolean): Promise<void> =>
	inApp() ? unwrap(commands.setScreenOn(on)).then(() => undefined) : Promise.resolve();
