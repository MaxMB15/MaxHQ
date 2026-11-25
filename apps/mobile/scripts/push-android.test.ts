// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { AndroidConfig } from "expo/config-plugins";
import { describe, expect, it } from "vitest";

import { EXPO_MESSAGING_SERVICE, withoutExpoMessaging } from "./push-android.ts";

type Manifest = AndroidConfig.Manifest.AndroidManifest;

// The part of the manifest Expo's Android template writes that the plugin
// changes.
const manifest = (): Manifest => ({
	manifest: {
		$: { "xmlns:android": "http://schemas.android.com/apk/res/android" },
		queries: [],
		application: [
			{
				$: { "android:name": ".MainApplication" },
				service: [{ $: { "android:name": "dev.example.Other" } }],
			},
		],
	},
});

describe("withoutExpoMessaging", () => {
	it("removes expo's messaging service and keeps the others", () => {
		const fitted = withoutExpoMessaging(manifest());
		expect(fitted.manifest.$["xmlns:tools"]).toBe("http://schemas.android.com/tools");
		expect(fitted.manifest.application?.[0]?.service).toEqual([
			{ $: { "android:name": "dev.example.Other" } },
			{ $: { "android:name": EXPO_MESSAGING_SERVICE, "tools:node": "remove" } },
		]);
	});

	it("leaves the manifest as it is on a second run", () => {
		const once = withoutExpoMessaging(manifest());
		const twice = withoutExpoMessaging(structuredClone(once));
		expect(twice).toEqual(once);
	});

	it("replaces an entry expo's service already has", () => {
		const start = manifest();
		start.manifest.application![0]!.service = [
			{ $: { "android:name": EXPO_MESSAGING_SERVICE, "android:exported": "false" } },
		];
		expect(withoutExpoMessaging(start).manifest.application?.[0]?.service).toEqual([
			{ $: { "android:name": EXPO_MESSAGING_SERVICE, "tools:node": "remove" } },
		]);
	});
});
