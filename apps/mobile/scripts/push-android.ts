// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A config plugin for Android push. The push module's own messaging
// service opens what maxd seals and hands the rest to expo-notifications,
// whose service it extends, so expo's entry comes out of the app's
// manifest: FCM then has one service to wake, ours. Without
// google-services.json Firebase never starts and neither service runs, so
// the entry comes out either way.

import type { ConfigPlugin } from "expo/config-plugins";
import { AndroidConfig, withAndroidManifest } from "expo/config-plugins";

type Manifest = AndroidConfig.Manifest.AndroidManifest;

/** The messaging service expo-notifications declares for itself. */
export const EXPO_MESSAGING_SERVICE =
	"expo.modules.notifications.service.ExpoFirebaseMessagingService";

/** The manifest with expo's messaging service marked for removal at merge. */
export const withoutExpoMessaging = (manifest: Manifest): Manifest => {
	const fitted = AndroidConfig.Manifest.ensureToolsAvailable(manifest);
	const app = AndroidConfig.Manifest.getMainApplicationOrThrow(fitted);
	const others = (app.service ?? []).filter(
		(service) => service.$["android:name"] !== EXPO_MESSAGING_SERVICE
	);
	app.service = [
		...others,
		{ $: { "android:name": EXPO_MESSAGING_SERVICE, "tools:node": "remove" } },
	];
	return fitted;
};

export const withAndroidPush: ConfigPlugin = (config) =>
	withAndroidManifest(config, (manifest) => {
		manifest.modResults = withoutExpoMessaging(manifest.modResults);
		return manifest;
	});
