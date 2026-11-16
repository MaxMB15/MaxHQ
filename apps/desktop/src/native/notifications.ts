// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Mac's own notifications: which ones show and sound, whether macOS
// lets them and the Dock badge, and a test one. The settings live with the app, which reads
// them before the page loads. In a browser there are none to show.

import type {
	NotificationAccess,
	NotificationPermission,
	NotificationPrefs,
} from "@maxhq/shared/bindings";
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

/** A build that can't show notifications, where nothing hides the badge. */
const UNAVAILABLE: NotificationAccess = { permission: "unavailable", badge: true };

/** Whether macOS lets the app show notifications and badge its icon. */
export const notificationAccess = (): Promise<NotificationAccess> =>
	inApp() ? unwrap(commands.notificationPermission()) : Promise.resolve(UNAVAILABLE);

/** Shows a test notification, asking macOS first if it hasn't asked yet. */
export const sendTestNotification = (): Promise<void> =>
	inApp() ? unwrap(commands.sendTestNotification()).then(() => undefined) : Promise.resolve();

/** Opens the app's page under Notifications in System Settings. */
export const openNotificationSettings = (): Promise<void> =>
	inApp() ? unwrap(commands.openNotificationSettings()).then(() => undefined) : Promise.resolve();

export type { NotificationAccess, NotificationPermission, NotificationPrefs };
