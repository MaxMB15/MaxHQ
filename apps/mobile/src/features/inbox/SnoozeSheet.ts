// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The snooze choices, in the system's own sheet: this phone's snooze
// options from Settings, "Until the limit resets" when the agent is at its
// limit, and Custom… for a date and time or a duration.

import { t } from "@maxhq/shared/i18n";

import type { AgentId, Timestamp } from "@maxhq/shared/bindings";
import { formatClock } from "@maxhq/shared/status/format";
import { changeInbox, type InboxNotices, showFailure } from "@maxhq/shared/ui/inbox-notice";
import { type InboxChoice, limitSnoozedText } from "@maxhq/shared/view/inbox-actions";
import { pickSnoozePreset, resolveSnoozePresets } from "@maxhq/shared/view/snooze";

import { choose } from "../../ui/choose";
import { requestCustomSnooze } from "./CustomSnooze";
import { inboxPrefs } from "./prefs";

const formatTime = (date: Date): string => formatClock(date.getTime());

/** Asks when to wake the agents, then snoozes them, with a notice to undo it. */
export const pickSnooze = (
	notices: InboxNotices,
	agents: readonly AgentId[],
	untilLimit: InboxChoice | null
): void => {
	const { snoozePresets: presets } = inboxPrefs.getState();
	const shown = resolveSnoozePresets(new Date(), { presets, formatTime });
	const snooze = (until: Timestamp) =>
		void changeInbox(notices, [{ agents, change: { change: "snooze", until } }]);
	choose(t("Snooze until"), [
		...shown.map((preset) => ({
			label: `${preset.label}, ${preset.whenLabel}`,
			pick: () => {
				const id = preset.id;
				const picked = pickSnoozePreset({
					shown,
					id,
					now: new Date(),
					presets,
					formatTime,
				});
				if (picked) snooze(picked.until);
				else showFailure(notices, t("That snooze time has passed. Choose another time."));
			},
		})),
		...(untilLimit
			? [
					{
						label: untilLimit.label,
						pick: () => void changeInbox(notices, untilLimit.steps, limitSnoozedText),
					},
				]
			: []),
		{
			label: t("Custom…"),
			pick: () =>
				void requestCustomSnooze().then((until) => {
					if (until !== null) snooze(until);
				}),
		},
	]);
};
