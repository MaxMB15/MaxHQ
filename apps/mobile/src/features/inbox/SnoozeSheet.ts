// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The snooze choices, in the system's own sheet: the default presets, and
// "Until the limit resets" when the agent is at its limit.

import { t } from "@maxhq/shared/i18n";

import type { AgentId } from "@maxhq/shared/bindings";
import { changeInbox, type InboxNotices } from "@maxhq/shared/ui/inbox-notice";
import { type InboxChoice, limitSnoozedText } from "@maxhq/shared/view/inbox-actions";
import { pickSnoozePreset, resolveSnoozePresets } from "@maxhq/shared/view/snooze";

import { choose } from "../../ui/choose";

/** Asks when to wake the agents, then snoozes them, with a notice to undo it. */
export const pickSnooze = (
	notices: InboxNotices,
	agents: readonly AgentId[],
	untilLimit: InboxChoice | null
): void => {
	const shown = resolveSnoozePresets(new Date());
	choose(t("Snooze until"), [
		...shown.map((preset) => ({
			label: `${preset.label}, ${preset.whenLabel}`,
			pick: () => {
				const picked = pickSnoozePreset({ shown, id: preset.id, now: new Date() });
				if (picked) {
					const until = picked.until;
					void changeInbox(notices, [{ agents, change: { change: "snooze", until } }]);
				}
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
	]);
};
