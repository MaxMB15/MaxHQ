// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sheet a long press on a phone's inbox row opens: the inbox menu for
// that agent, in a list of plain words, then Rename, then Delete for an
// agent MaxHQ holds. A sheet has no checkboxes, so auto-settle reads as
// what a tap does.

import { t } from "../i18n";

import type { AgentId } from "../bindings";

import type { InboxChoice, InboxMenu } from "./inbox-actions";

/** One line of the sheet. */
export type InboxSheetEntry =
	| { readonly kind: "change"; readonly label: string; readonly choice: InboxChoice }
	| {
			readonly kind: "snooze";
			readonly label: string;
			readonly agents: readonly AgentId[];
			/** "Until the limit resets", offered beside the times when it applies. */
			readonly untilLimit: InboxChoice | null;
	  }
	| { readonly kind: "rename"; readonly label: string }
	| { readonly kind: "delete"; readonly label: string };

/**
 * The sheet's lines, top to bottom: Pin or Unpin, Snooze or Wake, Settle
 * or Unsettle, auto-settle, continuing after a usage limit, Rename and
 * Delete. `menu` is null where the
 * agent's device keeps no inbox, which leaves Rename and Delete.
 */
export const inboxSheet = (menu: InboxMenu | null, deletable: boolean): InboxSheetEntry[] => {
	const entries: InboxSheetEntry[] = [];
	const change = (choice: InboxChoice | null, label = choice?.label) => {
		if (choice && label) entries.push({ kind: "change", label, choice });
	};
	if (menu) {
		change(menu.pin);
		if (menu.wake) change(menu.wake);
		else if (menu.snooze) {
			const { snooze: agents, untilLimit } = menu;
			entries.push({ kind: "snooze", label: t("Snooze"), agents, untilLimit });
		}
		change(menu.settle);
		change(menu.autoSettleOff, t("Turn off auto-settle"));
		change(menu.autoSettleOn, t("Turn on auto-settle"));
		change(menu.continueOff, t("Don't continue after the limit"));
		change(menu.continueOn, t("Continue after the limit"));
	}
	entries.push({ kind: "rename", label: t("Rename") });
	if (deletable) entries.push({ kind: "delete", label: t("Delete agent") });
	return entries;
};
