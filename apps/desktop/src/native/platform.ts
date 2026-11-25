// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { isTauri } from "@tauri-apps/api/core";
import { osOfUserAgent, type AppOs } from "@maxhq/shared/lib/os";

export type { AppOs } from "@maxhq/shared/lib/os";

export const inApp = (): boolean => isTauri();

let read: { readonly agent: string; readonly os: AppOs } | null = null;

/**
 * The system the app runs on, read from the web view's user agent: right
 * away and the same on every call, so the first frame is already drawn for
 * it. The only place the app asks which system it's on.
 */
export const appOs = (): AppOs => {
	const agent = navigator.userAgent;
	if (read?.agent !== agent) read = { agent, os: osOfUserAgent(agent) };
	return read.os;
};

export const onMac = (): boolean => appOs() === "macos";
export const onWindows = (): boolean => appOs() === "windows";
export const onLinux = (): boolean => appOs() === "linux";
