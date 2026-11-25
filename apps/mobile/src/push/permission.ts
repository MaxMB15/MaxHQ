// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether the system lets MaxHQ notify. The app asks once a device is
// paired, when a push has something to say, and fetches the token whenever
// the system allows it: an APNs token on iOS and an FCM token on Android.

import Constants from "expo-constants";
import * as Notifications from "expo-notifications";
import { createStore } from "zustand";

import { platform } from "../platform";
import { makeChannels } from "./channels";
import { pushStore } from "./kinds";
import { pushTopicOf } from "./topic";

/** What the system said last, or null where this build gets no pushes or before it's read. */
export const permissionStore = createStore<{
	readonly status: Notifications.PermissionStatus | null;
}>(() => ({ status: null }));

/** The topic pushes go to, when this build gets them. */
export const pushTopic = (): string | null =>
	pushTopicOf(Constants.expoConfig, platform.pushService);

const settle = async ({ status, granted }: Notifications.NotificationPermissionsStatus) => {
	permissionStore.setState({ status });
	if (!granted || pushStore.getState().token) return;
	const { data } = await Notifications.getDevicePushTokenAsync();
	if (typeof data === "string") pushStore.setState({ token: data });
};

const warn = (error: unknown) => console.warn("Couldn't read the notification permission", error);

/** Reads what the system says, without asking. */
export const checkPermission = () => {
	if (pushTopic()) Notifications.getPermissionsAsync().then(settle).catch(warn);
};

/** Asks the system to let MaxHQ notify. iOS shows its prompt only the first time. */
export const askToNotify = () => {
	if (!pushTopic()) return;
	makeChannels()
		.then(() => Notifications.requestPermissionsAsync())
		.then(settle)
		.catch(warn);
};
