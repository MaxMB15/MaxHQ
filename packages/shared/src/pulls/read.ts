// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A PR read's answer, as what was read or an error to show. A device that
// refuses, can't be reached or answers with something else gives the
// error, in words, so a query shows it the way it shows any failure.

import { t } from "../i18n";

import type { ActionOutcome } from "../bindings";
import type { ActionResult } from "../data/source";
import { errorText } from "../timeline/store";

/** What a read answers with when it reads what it asked for. */
export type Read<S extends ActionOutcome["status"]> = Extract<ActionOutcome, { status: S }>;

/** The outcome `status` names, or an error saying why there's none. */
export const readOf = <S extends ActionOutcome["status"]>(
	result: ActionResult,
	status: S
): Read<S> => {
	if (result.status === "error") throw new Error(errorText(result.error));
	const outcome = result.data;
	if (outcome.status === "refused") throw new Error(outcome.reason);
	if (outcome.status !== status)
		throw new Error(
			t("The device answered {{outcome_status}}.", { outcome_status: outcome.status })
		);
	return outcome as Read<S>;
};
