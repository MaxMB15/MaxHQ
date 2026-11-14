// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { test } from "node:test";
import { translationErrors } from "./check-translations.mjs";

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
