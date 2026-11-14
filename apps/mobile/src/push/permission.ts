// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether iOS lets MaxHQ notify. The app asks once a device is paired, when
// a push has something to say, and fetches the token whenever iOS allows it.

import Constants from "expo-constants";
import * as Notifications from "expo-notifications";
import { Platform } from "react-native";
import { createStore } from "zustand";

import { pushStore } from "./kinds";

/** What iOS said last, or null where this build sends no pushes or before it's read. */
export const permissionStore = createStore<{
	readonly status: Notifications.PermissionStatus | null;
}>(() => ({ status: null }));

/** The topic pushes go to, the app's bundle id, when this build sends them. */
export const pushTopic = (): string | null => {
	const config = Constants.expoConfig;
	if (Platform.OS !== "ios" || config?.extra?.push !== true) return null;
	return config.ios?.bundleIdentifier ?? null;
};

const settle = async ({ status, granted }: Notifications.NotificationPermissionsStatus) => {
	permissionStore.setState({ status });
	if (!granted || pushStore.getState().token) return;
	const { data } = await Notifications.getDevicePushTokenAsync();
	if (typeof data === "string") pushStore.setState({ token: data });
};

const warn = (error: unknown) => console.warn("Couldn't read the notification permission", error);

/** Reads what iOS says, without asking. */
export const checkPermission = () => {
	if (pushTopic()) Notifications.getPermissionsAsync().then(settle).catch(warn);
};

/** Asks iOS to let MaxHQ notify. It shows its prompt only the first time. */
export const askToNotify = () => {
	if (pushTopic()) Notifications.requestPermissionsAsync().then(settle).catch(warn);
};
