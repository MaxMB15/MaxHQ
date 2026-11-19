// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The line under an inbox row's title: which agent, where, and a word for
// a state its dot alone shows. Also the words at the row's end, and the
// actions only VoiceOver offers on the row.

import { t } from "@maxhq/shared/i18n";

import { type ActivityLook, agentRowMeta, type RowMeta } from "@maxhq/shared/status/activity";
import type { Agent, Timestamp, Vendor } from "@maxhq/shared/bindings";
import { VENDOR_NAME } from "@maxhq/shared/status/usage";
import { inboxOf } from "@maxhq/shared/view/inbox";
import type { ViewState } from "@maxhq/shared/view/replica";
import { snoozeWakeLabel } from "@maxhq/shared/view/snooze";
import type { RunChip } from "@maxhq/shared/workflow/select";

import type { SpokenAction } from "../../ui/SwipeRow";

const NOTE: Partial<Record<ActivityLook, string>> = {
	done_unread: "done",
	idle_cache_warm: "cache warm",
	waiting_for_limit_reset: "waiting for a limit",
	error: "stopped on an error",
	not_accessible: "can't reach it",
};

/** Such as "Claude on devbox, done". */
export const rowSubtitle = (vendor: Vendor, device: string, look: ActivityLook): string => {
	const key = NOTE[look];
	const note = key ? t(key) : undefined;
	const where = t("{{vendor}} on {{device}}", { vendor: VENDOR_NAME[vendor], device });
	return note ? `${where}, ${note}` : where;
};

/** Whether the row's title shows bright: it has news for you. */
export const loudRow = (look: ActivityLook): boolean =>
	look === "needs_input" || look === "done_unread" || look === "error";

/**
 * The words at the row's end: when a snoozed agent wakes, or what the
 * shared row helper says, such as "Input", "4m" or a muted "Asleep".
 */
export const phoneRowMeta = (view: ViewState, agent: Agent, now: Timestamp): RowMeta => {
	const until = inboxOf(agent).snoozed_until;
	return until !== null && until > now
		? { kind: "text", text: t("Wakes {{time}}", { time: snoozeWakeLabel(until, now) }) }
		: agentRowMeta(view, agent, now);
};

/**
 * What VoiceOver offers on a row besides its swipe actions: Open run when
 * the row shows a run chip, since it reads the row as one and can't reach
 * the chip.
 */
export const spokenRowActions = (
	chip: RunChip | null,
	openRun: (chip: RunChip) => void
): SpokenAction[] => (chip ? [{ label: t("Open run"), onPress: () => openRun(chip) }] : []);
