// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether this build gets pushes, and the topic they go to. It reads the
// built config, so it imports no React Native and the tests can load it.

import type { PushService } from "@maxhq/shared/bindings";

/** The parts of the built Expo config that say whether pushes reach this build. */
export type PushConfig = {
	readonly extra?: { readonly push?: unknown; readonly fcm?: unknown } | null;
	readonly ios?: { readonly bundleIdentifier?: string } | null;
	readonly android?: { readonly package?: string } | null;
};

/**
 * The topic pushes through `service` go to, or null when this build gets
 * none: the bundle id for APNs, which only a signed build gets pushes on,
 * and the package name for FCM, which needs Firebase's file in the build.
 */
export const pushTopicOf = (
	config: PushConfig | null | undefined,
	service: PushService
): string | null => {
	if (service === "apns") {
		return config?.extra?.push === true ? (config.ios?.bundleIdentifier ?? null) : null;
	}
	return config?.extra?.fcm === true ? (config.android?.package ?? null) : null;
};
