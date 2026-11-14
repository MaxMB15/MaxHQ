// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where the page runs, read when asked so a test can change it.

import { isTauri } from "@tauri-apps/api/core";

/** Whether the page runs inside the app, rather than in a browser. */
export const inApp = (): boolean => isTauri();

/** Whether the page runs on macOS. */
export const onMac = (): boolean => navigator.userAgent.includes("Mac");

/** Whether the page runs on Windows. */
export const onWindows = (): boolean => navigator.userAgent.includes("Windows");
