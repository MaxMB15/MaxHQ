// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Skipping what an agent waits on you for. Dismiss on a question card turns
// its questions down and stops the turn, as Esc does in the CLI, so a
// message you queued starts the next turn. Send now in the composer does
// that for everything waiting on the agent and sends your message as the
// next turn. Either way the card closes as dismissed: turned down, with
// nothing answered or decided. A maxd from before `DISMISS_SINCE` can't
// dismiss a card, though it still stops the turn for Send now.

import { t } from "../i18n";
import { type AttentionAnswer, DISMISS_SINCE, type DeviceId } from "../bindings";
import { kindName, type AgentKind } from "../status/agent-kind";
import type { FeedCard } from "../timeline/output";
import type { ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";

/** The answer that dismisses a prompt. */
export const DISMISS: AttentionAnswer = { type: "dismiss" };

/** Whether the maxd on `device` can dismiss a prompt. */
export const takesDismiss = (view: ViewState, device: DeviceId): boolean => {
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	return protocol !== null && protocol >= DISMISS_SINCE;
};

/**
 * Why Dismiss is off for an agent on `device`, or null when it's on. With
 * no device known, the answer's own failure says what went wrong.
 */
export const dismissBlocked = (view: ViewState, device: DeviceId | null): string | null =>
	device === null || takesDismiss(view, device)
		? null
		: t("Dismiss needs a newer maxd on {{device}}.", { device: deviceName(view, device) });

/** Whether a card was dismissed: turned down with nothing answered or decided. */
export const wasDismissed = (card: FeedCard): boolean =>
	card.status === "declined" &&
	(card.kind === "question" ? card.answers === null : card.decided === null);

/**
 * What a card says once its Dismiss went through, until maxd closes it as
 * dismissed: the agent's turn is on its way to a stop.
 */
export const dismissSent = (kind: AgentKind): string =>
	t("Dismissed. Stopping {{vendor}}'s turn.", { vendor: kindName(kind) });

/** The words for Dismiss and Send now, in the interface's language. */
export const SKIP_WORDS = {
	get dismiss() {
		return t("Dismiss");
	},
	get dismissTip() {
		return t("Skip these questions and stop the turn");
	},
	get dismissed() {
		return t("Dismissed");
	},
	get sendNow() {
		return t("Send now");
	},
	get sendNowTip() {
		return t("Skips what's waiting and sends this now");
	},
};
