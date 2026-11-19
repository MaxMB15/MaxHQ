// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent list's settings that belong to this phone, as the Mac keeps
// its own: whether busy agents fold into a Working shelf, and the snooze
// menu's choices. The file can hold anything, so a read checks what it got
// and falls back to the defaults.

import {
	DEFAULT_SNOOZE_PRESETS,
	isSnoozePresetList,
	type SnoozePresetSetting,
} from "@maxhq/shared/view/snooze";

/** The agent list's own settings. */
export type InboxPrefs = {
	/** Whether busy agents fold into a Working shelf. */
	readonly workingShelf: boolean;
	/** The snooze menu's choices. */
	readonly snoozePresets: readonly SnoozePresetSetting[];
};

export const DEFAULT_INBOX_PREFS: InboxPrefs = {
	workingShelf: false,
	snoozePresets: DEFAULT_SNOOZE_PRESETS,
};

/** The settings in a kept file's text, or the defaults for what isn't right. */
export const inboxPrefsOf = (text: string | null): InboxPrefs => {
	if (!text) return DEFAULT_INBOX_PREFS;
	try {
		const kept: unknown = JSON.parse(text);
		if (typeof kept !== "object" || kept === null) return DEFAULT_INBOX_PREFS;
		const { workingShelf, snoozePresets } = kept as Record<string, unknown>;
		return {
			workingShelf:
				typeof workingShelf === "boolean" ? workingShelf : DEFAULT_INBOX_PREFS.workingShelf,
			snoozePresets: isSnoozePresetList(snoozePresets)
				? snoozePresets
				: DEFAULT_INBOX_PREFS.snoozePresets,
		};
	} catch {
		return DEFAULT_INBOX_PREFS;
	}
};
