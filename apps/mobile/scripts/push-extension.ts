// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A config plugin that adds the push extension to the iOS project at
// prebuild. The extension's Swift lives in targets/push. It opens each
// push with the phone's push key, which it reads from the Keychain group
// it shares with the app, so it gets the app's access group too.

import { copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

import type { ConfigPlugin } from "expo/config-plugins";
import { withDangerousMod, withXcodeProject } from "expo/config-plugins";

export const PUSH_TARGET = "MaxHQPush";

export type PushExtension = {
	bundleIdentifier: string;
	teamId: string;
	keychainGroup: string;
	version: string;
	buildNumber: string;
};

// The Swift files the target builds, from targets/push, and their names
// in the target.
const SOURCES = ["NotificationService.swift", "Sources/PushOpen/PushOpen.swift"];
const FILES = SOURCES.map((source) => basename(source));

const DEPLOYMENT_TARGET = "15.1";

export const infoPlist = (): string => `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>$(DEVELOPMENT_LANGUAGE)</string>
	<key>CFBundleDisplayName</key>
	<string>${PUSH_TARGET}</string>
	<key>CFBundleExecutable</key>
	<string>$(EXECUTABLE_NAME)</string>
	<key>CFBundleIdentifier</key>
	<string>$(PRODUCT_BUNDLE_IDENTIFIER)</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>$(PRODUCT_NAME)</string>
	<key>CFBundlePackageType</key>
	<string>$(PRODUCT_BUNDLE_PACKAGE_TYPE)</string>
	<key>CFBundleShortVersionString</key>
	<string>$(MARKETING_VERSION)</string>
	<key>CFBundleVersion</key>
	<string>$(CURRENT_PROJECT_VERSION)</string>
	<key>NSExtension</key>
	<dict>
		<key>NSExtensionPointIdentifier</key>
		<string>com.apple.usernotifications.service</string>
		<key>NSExtensionPrincipalClass</key>
		<string>$(PRODUCT_MODULE_NAME).NotificationService</string>
	</dict>
</dict>
</plist>
`;

export const entitlements = (
	keychainGroup: string
): string => `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>keychain-access-groups</key>
	<array>
		<string>${keychainGroup}</string>
	</array>
</dict>
</plist>
`;

// The extension's build settings, beside the ones addTarget writes.
export const buildSettings = (push: PushExtension): Record<string, string> => ({
	CODE_SIGN_ENTITLEMENTS: `${PUSH_TARGET}/${PUSH_TARGET}.entitlements`,
	CODE_SIGN_STYLE: "Automatic",
	CURRENT_PROJECT_VERSION: push.buildNumber,
	DEVELOPMENT_TEAM: push.teamId,
	GENERATE_INFOPLIST_FILE: "NO",
	INFOPLIST_FILE: `${PUSH_TARGET}/Info.plist`,
	IPHONEOS_DEPLOYMENT_TARGET: DEPLOYMENT_TARGET,
	MARKETING_VERSION: push.version,
	SWIFT_VERSION: "5.0",
	TARGETED_DEVICE_FAMILY: '"1,2"',
});

// Writes the extension's files into ios/MaxHQPush.
const withFiles: ConfigPlugin<PushExtension> = (config, push) =>
	withDangerousMod(config, [
		"ios",
		(mod) => {
			const appDir = mod.modRequest.projectRoot;
			const dir = join(mod.modRequest.platformProjectRoot, PUSH_TARGET);
			mkdirSync(dir, { recursive: true });
			SOURCES.forEach((source, i) =>
				copyFileSync(join(appDir, "targets/push", source), join(dir, FILES[i]!))
			);
			writeFileSync(join(dir, "Info.plist"), infoPlist());
			writeFileSync(
				join(dir, `${PUSH_TARGET}.entitlements`),
				entitlements(push.keychainGroup)
			);
			return mod;
		},
	]);

// Adds the target, its sources and its settings, once.
const withTarget: ConfigPlugin<PushExtension> = (config, push) =>
	withXcodeProject(config, (mod) => {
		const project = mod.modResults;
		if (project.pbxTargetByName(PUSH_TARGET)) return mod;
		const group = project.addPbxGroup(
			[...FILES, "Info.plist", `${PUSH_TARGET}.entitlements`],
			PUSH_TARGET,
			PUSH_TARGET
		);
		project.addToPbxGroup(group.uuid, project.getFirstProject().firstProject.mainGroup);
		const target = project.addTarget(
			PUSH_TARGET,
			"app_extension",
			PUSH_TARGET,
			push.bundleIdentifier
		);
		project.addBuildPhase(FILES, "PBXSourcesBuildPhase", "Sources", target.uuid);
		project.addBuildPhase([], "PBXResourcesBuildPhase", "Resources", target.uuid);
		project.addBuildPhase([], "PBXFrameworksBuildPhase", "Frameworks", target.uuid);
		const configurations = project.pbxXCBuildConfigurationSection();
		for (const key of Object.keys(configurations)) {
			const each = configurations[key];
			if (each?.buildSettings?.PRODUCT_NAME === `"${PUSH_TARGET}"`) {
				Object.assign(each.buildSettings, buildSettings(push));
			}
		}
		project.addTargetAttribute("DevelopmentTeam", push.teamId, target);
		return mod;
	});

export const withPushExtension: ConfigPlugin<PushExtension> = (config, push) =>
	withTarget(withFiles(config, push), push);
