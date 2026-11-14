// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The CLI login calls of the sample data and of tests. A Claude login gives
// a page and takes the code you paste back. A Codex login gives a page, or
// a code to enter with a device code sign-in, and a Cursor login gives a
// page. Both sign in by themselves a few seconds later, the way they do
// once you finish on the page. Nothing here runs a CLI or opens a page.

import type { Account, AccountId, SignInId } from "../bindings";
import type { CliLoginCalls } from "../settings/cli-sign-in";
import type { ActionResult } from "./source";

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

const NO_SIGN_IN = "That sign-in isn't there anymore.";

/** The CLI login calls of a device that can't sign in again. */
export const noCliLogins: CliLoginCalls = {
	beginCliLogin: async () => refused("This device can't sign in again from MaxHQ."),
	finishCliLogin: async () => refused(NO_SIGN_IN),
	checkCliLogin: async () => refused(NO_SIGN_IN),
	cancelCliLogin: async () => DONE,
};

/** The page a sample Claude login gives. */
export const MOCK_CLAUDE_PAGE = "https://claude.com/cai/oauth/authorize?code=true";
/** The page a sample Codex login gives. */
export const MOCK_CODEX_PAGE = "https://auth.openai.com/oauth/authorize";
/** The page a sample Codex device code sign-in gives. */
export const MOCK_CODEX_DEVICE_PAGE = "https://auth.openai.com/codex/device";
/** The code a sample Codex device code sign-in gives. */
export const MOCK_CODEX_CODE = "ABCD-1234";
/** The page a sample Cursor login gives. */
export const MOCK_CURSOR_PAGE = "https://cursor.com/loginDeepControl?mode=login&redirectTarget=cli";
/** How long a sample Codex or Cursor login takes to sign in by itself. */
export const CLI_SIGN_IN_MS = 3000;

type Round = {
	readonly account: AccountId;
	signedIn: boolean;
	timer?: ReturnType<typeof setTimeout>;
};

/**
 * The sample CLI logins. `accountOf` finds an account, and `signedIn` runs
 * when a login goes through, as the device's next usage check would.
 */
export const mockCliLogins = (options: {
	readonly accountOf: (id: AccountId) => Account | undefined;
	readonly signedIn: (id: AccountId) => void;
}): { readonly calls: CliLoginCalls } => {
	const rounds = new Map<SignInId, Round>();
	let count = 0;

	const signIn = (round: Round) => {
		round.signedIn = true;
		options.signedIn(round.account);
	};

	const state = (round: Round): ActionResult => ({
		status: "ok",
		data: {
			status: "cli_login_state",
			state: { state: round.signedIn ? "signed_in" : "waiting" },
		},
	});

	const calls: CliLoginCalls = {
		beginCliLogin: async (_device, id, deviceCode) => {
			const account = options.accountOf(id);
			if (!account) return refused("That account isn't there anymore.");
			if (account.vendor === "acp") {
				return refused("Only a Claude, Codex or Cursor login signs in again from MaxHQ.");
			}
			count += 1;
			const signInId = `cli-sign-in-${count}`;
			const round: Round = { account: id, signedIn: false };
			rounds.set(signInId, round);
			if (account.vendor === "claude") {
				return {
					status: "ok",
					data: {
						status: "cli_login",
						sign_in: signInId,
						url: MOCK_CLAUDE_PAGE,
						user_code: null,
						paste: true,
					},
				};
			}
			round.timer = setTimeout(() => signIn(round), CLI_SIGN_IN_MS);
			// Cursor never asks for a device code.
			const code = deviceCode && account.vendor === "codex";
			const url =
				account.vendor === "cursor"
					? MOCK_CURSOR_PAGE
					: code
						? MOCK_CODEX_DEVICE_PAGE
						: MOCK_CODEX_PAGE;
			return {
				status: "ok",
				data: {
					status: "cli_login",
					sign_in: signInId,
					url,
					user_code: code ? MOCK_CODEX_CODE : null,
					paste: false,
				},
			};
		},
		finishCliLogin: async (_device, signInId, code) => {
			const round = rounds.get(signInId);
			if (!round) return refused(NO_SIGN_IN);
			if (code.trim() === "") return refused("Paste the code from the page first.");
			if (!round.signedIn) signIn(round);
			return state(round);
		},
		checkCliLogin: async (_device, signInId) => {
			const round = rounds.get(signInId);
			return round ? state(round) : refused(NO_SIGN_IN);
		},
		cancelCliLogin: async (_device, signInId) => {
			clearTimeout(rounds.get(signInId)?.timer);
			rounds.delete(signInId);
			return DONE;
		},
	};

	return { calls };
};
