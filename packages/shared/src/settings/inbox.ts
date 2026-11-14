// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The inbox settings a device keeps: how many idle days pass before an
// agent settles on its own, and whether an agent that stops at a usage
// limit snoozes until the limit resets. A project can set its own days.
// A `maxd` from before inboxes keeps neither, so the forms ask first.

import { t } from "../i18n";

import type { DeviceId } from "../bindings";
import type { ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

/** The idle days before auto-settle, unless a device says otherwise. */
export const DEFAULT_SETTLE_DAYS = 5;

/** The most idle days a device or project may wait. */
export const MAX_SETTLE_DAYS = 90;

/** Whether a device's `maxd` keeps inboxes, as its hello said. */
export const keepsInboxes = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.inbox === true;

/** The days typed in, or why they don't read as days. 0 means never. */
export const parseSettleDays = (
	text: string
): { ok: true; days: number } | { ok: false; reason: string } => {
	const typed = text.trim();
	const days = /^\d{1,3}$/.test(typed) ? Number(typed) : NaN;
	if (!(days >= 0 && days <= MAX_SETTLE_DAYS)) {
		return {
			ok: false,
			reason: t(
				"{{typed___________Nothing____typed}} isn't a number of days. Use 0 to {{MAX_SETTLE_DAYS}}.",
				{
					typed___________Nothing____typed: typed === "" ? "Nothing" : typed,
					MAX_SETTLE_DAYS: MAX_SETTLE_DAYS,
				}
			),
		};
	}
	return { ok: true, days };
};

/** The days in words, such as "5 days", or "Never" for 0. */
export const settleDaysText = (days: number): string =>
	days === 0 ? t("Never") : `${days} ${days === 1 ? "day" : "days"}`;
