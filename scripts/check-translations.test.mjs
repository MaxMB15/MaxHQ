// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { test } from "node:test";
import { translationErrors, variantErrors } from "./check-translations.mjs";

test("reports missing English keys and interpolation arguments", () => {
	assert.deepEqual(
		translationErrors(
			{ "Hello {{name}}": "Hello {{name}}", Close: "Close" },
			{ "Hello {{name}}": "Hallo {{person}}" }
		),
		["Hello {{name}}: interpolation differs", "Close: missing translation"]
	);
});
test("allows reordered interpolation and additional locale plural forms", () => {
	assert.deepEqual(
		translationErrors(
			{ "{{count}} item_one": "{{count}} item", "{{count}} item_other": "{{count}} items" },
			{
				"{{count}} item_one": "article {{count}}",
				"{{count}} item_other": "articles {{count}}",
				"{{count}} item_many": "articles {{count}}",
			}
		),
		[]
	);
});
test("rejects empty translations", () => {
	assert.deepEqual(translationErrors({ Close: "Close" }, { Close: " " }), [
		"Close: missing translation",
	]);
});
test("accepts a system variant with its base and its sibling", () => {
	assert.deepEqual(
		variantErrors({
			"On {{name}}'s Mac": "On {{name}}'s Mac",
			"On {{name}}'s Mac_windows": "On {{name}}'s PC",
			"On {{name}}'s Mac_linux": "On {{name}}'s computer",
		}),
		[]
	);
});
test("reports a missing sibling, a missing base and a macOS variant", () => {
	assert.deepEqual(
		variantErrors({
			"This Mac": "This Mac",
			"This Mac_windows": "This PC",
			Finder_linux: "the file manager",
			Dock_macos: "Dock",
		}),
		[
			'This Mac_windows: missing "This Mac_linux"',
			'Finder_linux: no base key "Finder"',
			"Dock_macos: macOS reads the base key, so drop this variant",
		]
	);
});
test("reports a variant whose arguments differ from the base", () => {
	assert.deepEqual(
		variantErrors({
			"{{name}} on this Mac": "{{name}} on this Mac",
			"{{name}} on this Mac_windows": "{{device}} on this PC",
			"{{name}} on this Mac_linux": "{{name}} on this computer",
		}),
		["{{name}} on this Mac_windows: interpolation differs from the base key"]
	);
});
test("needs the singular of a plural key's variants", () => {
	assert.deepEqual(
		variantErrors({
			"{{count}} Macs": "{{count}} Macs",
			"{{count}} Macs_one": "{{count}} Mac",
			"{{count}} Macs_windows": "{{count}} PCs",
			"{{count}} Macs_linux": "{{count}} computers",
			"{{count}} Macs_linux_one": "{{count}} computer",
		}),
		[
			'{{count}} Macs_windows: missing "{{count}} Macs_windows_one"',
			'{{count}} Macs_linux_one: missing "{{count}} Macs_windows_one"',
		]
	);
});
test("checks an Android variant against its iOS base, with no sibling", () => {
	assert.deepEqual(
		variantErrors({
			"Turn them on in iOS Settings.": "Turn them on in iOS Settings.",
			"Turn them on in iOS Settings._android": "Turn them on in Android settings.",
			"Open {{name}} in TestFlight": "Open {{name}} in TestFlight",
			"Open {{name}} in TestFlight_android": "Download {{app}}",
			"Files app_android": "your files",
			Photos_ios: "Photos",
		}),
		[
			"Open {{name}} in TestFlight_android: interpolation differs from the base key",
			'Files app_android: no base key "Files app"',
			"Photos_ios: iOS reads the base key, so drop this variant",
		]
	);
});
