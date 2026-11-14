// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Mac's own notifications: which ones show and sound, whether macOS
// lets them, and a test one. The settings live with the app, which reads
// them before the page loads. In a browser there are none to show.

import type { NotificationPermission, NotificationPrefs } from "@maxhq/shared/bindings";
import { DEFAULT_KINDS } from "@maxhq/shared/push/push";
import { commands } from "@/data/tauri-commands";

import { unwrap } from "./failure";
import { inApp } from "./platform";

/** Every kind shows, and only the ones that wait on you play a sound. */
export const DEFAULT_NOTIFICATION_PREFS: NotificationPrefs = {
	show: true,
	kinds: [...DEFAULT_KINDS],
	prompt_sound: true,
	other_sound: false,
};

/** The settings now, or the defaults outside the app. */
export const notificationPrefs = (): Promise<NotificationPrefs> =>
	inApp() ? commands.notificationPrefs() : Promise.resolve(DEFAULT_NOTIFICATION_PREFS);

/** Saves the settings. The next alert follows them. */
export const setNotificationPrefs = (prefs: NotificationPrefs): Promise<void> =>
	inApp()
		? unwrap(commands.setNotificationPrefs(prefs)).then(() => undefined)
		: Promise.resolve();

/** Whether macOS lets the app show notifications. */
export const notificationPermission = (): Promise<NotificationPermission> =>
	inApp() ? unwrap(commands.notificationPermission()) : Promise.resolve("unavailable");

/** Shows a test notification, asking macOS first if it hasn't asked yet. */
export const sendTestNotification = (): Promise<void> =>
	inApp() ? unwrap(commands.sendTestNotification()).then(() => undefined) : Promise.resolve();

/** Opens the app's page under Notifications in System Settings. */
export const openNotificationSettings = (): Promise<void> =>
	inApp() ? unwrap(commands.openNotificationSettings()).then(() => undefined) : Promise.resolve();

export type { NotificationPermission, NotificationPrefs };
