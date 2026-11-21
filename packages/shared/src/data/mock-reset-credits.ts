// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The banked reset calls of the sample data and of tests. A reset takes
// one from the account's count and empties its windows, as Codex does, and
// an account with none banked has nothing to spend.

import type { Account, AccountId } from "../bindings";
import type { ResetCreditCalls } from "../status/reset-credits";

/** The banked reset calls over the sample accounts. */
export const mockResetCredits = (accounts: {
	/** The account as the view has it now. */
	readonly get: (id: AccountId) => Account | undefined;
	/** Shows the account changed. */
	readonly set: (account: Account) => void;
}): ResetCreditCalls => ({
	useResetCredit: async (id) => {
		const account = accounts.get(id);
		if (!account)
			return { status: "error", error: { kind: "not_found", what: "This account" } };
		const usage = account.usage;
		const count = usage?.reset_credits ?? 0;
		if (account.vendor !== "codex" || !usage || count <= 0) {
			return { status: "ok", data: { status: "reset_credit", outcome: "no_credit" } };
		}
		accounts.set({
			...account,
			usage: {
				...usage,
				reset_credits: count - 1,
				windows: usage.windows.map((w) => ({ ...w, used_percent: 0 })),
			},
		});
		return { status: "ok", data: { status: "reset_credit", outcome: "reset" } };
	},
});
