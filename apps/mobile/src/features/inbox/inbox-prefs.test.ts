// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { DEFAULT_SNOOZE_PRESETS } from "@maxhq/shared/view/snooze";

import { DEFAULT_INBOX_PREFS, inboxPrefsOf } from "./inbox-prefs";

describe("inboxPrefsOf", () => {
	it("reads what it kept", () => {
		const snoozePresets = [{ kind: "after", minutes: 90 }];
		expect(inboxPrefsOf(JSON.stringify({ workingShelf: true, snoozePresets }))).toEqual({
			workingShelf: true,
			snoozePresets,
		});
	});

	it("falls back to the defaults for a missing or broken file", () => {
		expect(inboxPrefsOf(null)).toEqual(DEFAULT_INBOX_PREFS);
		expect(inboxPrefsOf("{not json")).toEqual(DEFAULT_INBOX_PREFS);
		expect(inboxPrefsOf("12")).toEqual(DEFAULT_INBOX_PREFS);
	});

	it("keeps the good settings and drops a bad one", () => {
		const kept = { workingShelf: "yes", snoozePresets: [{ kind: "after", minutes: 0 }] };
		expect(inboxPrefsOf(JSON.stringify(kept))).toEqual({
			workingShelf: false,
			snoozePresets: DEFAULT_SNOOZE_PRESETS,
		});
	});
});
