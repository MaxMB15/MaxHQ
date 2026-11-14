// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What app.config.ts needs to know about signing and the two variants. The
// Team ID and bundle ID live in apps/mobile/ios.local.json, which git ignores,
// so no real ID is ever in the repo. The variants are development and
// production.

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

export type AppVariant = "development" | "production";

export type IosLocal = { teamId: string; bundleId: string };

export const LOCAL_FILE = "apps/mobile/ios.local.json";
export const FALLBACK_BUNDLE_ID = "com.example.maxhq";

// The scripts set this for any build that goes to a real iPhone. Without it a
// missing local file means a simulator build, which needs no signing.
export const DEVICE_ENV = "MAXHQ_IOS_DEVICE";

export const MISSING_LOCAL = `Add ${LOCAL_FILE} with your Team ID and bundle ID before building for a device.`;

const TEAM_ID = /^[A-Z0-9]{10}$/;
const BUNDLE_ID = /^[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+$/;

const VARIANTS = {
	development: { name: "MaxHQ Dev", scheme: "maxhq-dev", suffix: ".dev" },
	production: { name: "MaxHQ", scheme: "maxhq", suffix: "" },
} as const;

// Production unless asked otherwise.
export const resolveVariant = (value: string | undefined): AppVariant =>
	value === "development" ? "development" : "production";

// Checks the local file's text. Each error names the file and never echoes
// what's in it.
export const parseIosLocal = (text: string): IosLocal => {
	let value: unknown;
	try {
		value = JSON.parse(text);
	} catch {
		throw new Error(
			`${LOCAL_FILE} isn't valid JSON; copy ios.local.example.json and fill it in.`
		);
	}
	const { teamId, bundleId } = (value ?? {}) as Record<string, unknown>;
	if (typeof teamId !== "string" || !TEAM_ID.test(teamId)) {
		throw new Error(`${LOCAL_FILE} needs "teamId" set to your 10-character Apple Team ID.`);
	}
	if (typeof bundleId !== "string" || !BUNDLE_ID.test(bundleId)) {
		throw new Error(
			`${LOCAL_FILE} needs "bundleId" set to a reverse-DNS ID such as com.example.maxhq.`
		);
	}
	return { teamId, bundleId };
};

// The local file, or null when it isn't there.
export const readIosLocal = (appDir: string): IosLocal | null => {
	const file = join(appDir, "ios.local.json");
	return existsSync(file) ? parseIosLocal(readFileSync(file, "utf8")) : null;
};

export type IosConfig = {
	name: string;
	scheme: string;
	bundleIdentifier: string;
	appleTeamId?: string;
	entitlements: Record<string, unknown>;
	infoPlist: Record<string, string>;
	push: boolean;
	appGroup: string | null;
};

// iOS asks once before the app reaches a device's network access on the
// local network, and shows this line.
const INFO_PLIST = {
	NSLocalNetworkUsageDescription: "MaxHQ reaches your Mac and other devices on your network.",
};

// The signed build gets push, an app group and a Keychain access group, all
// from the variant's bundle ID. The unsigned simulator fallback gets none.
export const iosConfig = (
	variant: AppVariant,
	local: IosLocal | null,
	device: boolean
): IosConfig => {
	const { name, scheme, suffix } = VARIANTS[variant];
	if (!local) {
		if (device) throw new Error(MISSING_LOCAL);
		return {
			name,
			scheme,
			bundleIdentifier: FALLBACK_BUNDLE_ID + suffix,
			entitlements: {},
			infoPlist: INFO_PLIST,
			push: false,
			appGroup: null,
		};
	}
	const bundleIdentifier = local.bundleId + suffix;
	const appGroup = `group.${bundleIdentifier}`;
	return {
		name,
		scheme,
		bundleIdentifier,
		appleTeamId: local.teamId,
		entitlements: {
			"aps-environment": variant === "development" ? "development" : "production",
			"com.apple.security.application-groups": [appGroup],
			"keychain-access-groups": [`$(AppIdentifierPrefix)${bundleIdentifier}`],
			// Prompts come as time-sensitive, so they break through Focus.
			"com.apple.developer.usernotifications.time-sensitive": true,
		},
		infoPlist: INFO_PLIST,
		push: true,
		appGroup,
	};
};
