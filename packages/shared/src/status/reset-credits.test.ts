// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	RESET_CREDITS_SINCE,
	type Account,
	type ActionOutcome,
	type Request,
	type Usage,
} from "../bindings";
import { mockResetCredits } from "../data/mock-reset-credits";
import type { Ask, Result } from "../data/request-source";
import { account, entities, link, NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";
import {
	canUseReset,
	resetCreditCalls,
	resetCredits,
	resetCreditsLine,
	resetLine,
	resetQuestion,
	resetRunning,
	resetSpends,
} from "./reset-credits";

const usage = (reset_credits: number | null): Usage => ({
	checked_at: NOW,
	source: "codex_app_server",
	windows: [{ scope: null, window_minutes: 300, used_percent: 80, resets_at: NOW + 3_600_000 }],
	reset_credits,
});

const codex = (count: number | null, over: Partial<Account> = {}): Account =>
	account({
		id: "acct-codex",
		vendor: "codex",
		label: "Codex",
		config_dir: "/Users/someone/.codex",
		usage: usage(count),
		...over,
	});

/** A view whose Mac link speaks `protocol`. */
const viewWith = (protocol: number | null, accounts: Account[]): ViewState =>
	applySnapshot({
		seq: 1,
		entities: entities({
			links: [link({ id: "mac", device: "dev-mac", route: { type: "local" }, protocol })],
			accounts,
		}),
	});

describe("banked resets", () => {
	it("says how many a Codex account has banked", () => {
		expect(resetCreditsLine(codex(1))).toBe("1 reset banked");
		expect(resetCreditsLine(codex(2))).toBe("2 resets banked");
		expect(resetCreditsLine(codex(1200))).toBe("1,200 resets banked");
	});

	it("says nothing when none are banked or nothing said", () => {
		expect(resetCreditsLine(codex(0))).toBeUndefined();
		expect(resetCreditsLine(codex(null))).toBeUndefined();
		expect(resetCreditsLine(codex(null, { usage: null }))).toBeUndefined();
		expect(resetCredits(codex(null, { usage: null }))).toBeNull();
		// Only Codex banks resets.
		const claude = codex(3, { vendor: "claude", label: "Claude" });
		expect(resetCredits(claude)).toBeNull();
		expect(resetCreditsLine(claude)).toBeUndefined();
	});

	it("shows Use reset only with a reset banked and a maxd that can spend it", () => {
		const two = codex(2);
		expect(canUseReset(viewWith(RESET_CREDITS_SINCE, [two]), two)).toBe(true);
		expect(canUseReset(viewWith(RESET_CREDITS_SINCE + 3, [two]), two)).toBe(true);
		// A maxd from before the request, or one that hasn't said.
		expect(canUseReset(viewWith(RESET_CREDITS_SINCE - 1, [two]), two)).toBe(false);
		expect(canUseReset(viewWith(null, [two]), two)).toBe(false);
		// None to spend.
		const none = codex(0);
		expect(canUseReset(viewWith(RESET_CREDITS_SINCE, [none]), none)).toBe(false);
		const unknown = codex(null);
		expect(canUseReset(viewWith(RESET_CREDITS_SINCE, [unknown]), unknown)).toBe(false);
	});

	it("leaves Use reset off for a login maxd brokers or another vendor", () => {
		const brokered = codex(2, { login: "chat_gpt_sign_in" });
		expect(canUseReset(viewWith(RESET_CREDITS_SINCE, [brokered]), brokered)).toBe(false);
		const claude = codex(2, { vendor: "claude" });
		expect(canUseReset(viewWith(RESET_CREDITS_SINCE, [claude]), claude)).toBe(false);
		// A device with no link at all.
		const elsewhere = codex(2, { device: "dev-gone" });
		expect(canUseReset(viewWith(RESET_CREDITS_SINCE, [elsewhere]), elsewhere)).toBe(false);
	});

	it("asks before spending one, and says when it's the last", () => {
		expect(resetQuestion(2)).toEqual({
			title: "Use a Codex reset?",
			detail: "This spends 1 of your 2 banked resets and resets this account's Codex limits now. It can't be undone.",
			action: "Use reset",
		});
		expect(resetQuestion(1).detail).toBe(
			"This spends your only banked reset and resets this account's Codex limits now. It can't be undone."
		);
		expect(resetRunning()).toBe("Using a reset…");
	});
});

describe("a spend's line", () => {
	const answer = (outcome: "reset" | "nothing_to_reset" | "no_credit" | "already_redeemed") =>
		resetLine({ status: "ok", data: { status: "reset_credit", outcome } });

	it("says what each outcome came to", () => {
		expect(answer("reset")).toEqual({
			text: "Reset used. This account's Codex limits start over now.",
			tone: "done",
		});
		// The same click that already went through spent it.
		expect(answer("already_redeemed")).toEqual(answer("reset"));
		expect(answer("nothing_to_reset")).toEqual({
			text: "No limit needs a reset right now, so nothing was spent.",
			tone: "quiet",
		});
		expect(answer("no_credit")).toEqual({ text: "No resets left.", tone: "quiet" });
	});

	it("says why one failed", () => {
		expect(
			resetLine({
				status: "ok",
				data: {
					status: "refused",
					reason: "A reset for this account is already in progress. Wait for it to finish.",
				},
			})
		).toEqual({
			text: "Couldn't use a reset. A reset for this account is already in progress. Wait for it to finish.",
			tone: "failed",
		});
		expect(
			resetLine({ status: "error", error: { kind: "not_connected", device: "Mac mini" } })
		).toEqual({ text: "Couldn't use a reset. Mac mini isn't connected.", tone: "failed" });
		expect(resetLine({ status: "ok", data: { status: "done" } })).toEqual({
			text: "The device gave an answer MaxHQ doesn't know. Check the count before trying again.",
			tone: "failed",
		});
	});
});

describe("the calls", () => {
	it("asks for one spend for the account", async () => {
		const asked: Request[] = [];
		const ask: Ask = async (request) => {
			asked.push(request);
			return { status: "ok", data: { status: "reset_credit", outcome: "reset" } } as never;
		};
		const calls = resetCreditCalls(ask);
		expect(await calls.useResetCredit("acct-codex")).toEqual({
			status: "ok",
			data: { status: "reset_credit", outcome: "reset" },
		});
		expect(asked).toEqual([{ kind: "use_reset_credit", account: "acct-codex" }]);
	});

	it("spends a sample reset and empties the windows", async () => {
		let shown: Account | undefined = codex(2);
		const calls = mockResetCredits({
			get: (id) => (shown?.id === id ? shown : undefined),
			set: (next) => {
				shown = next;
			},
		});
		expect(await calls.useResetCredit("acct-codex")).toEqual({
			status: "ok",
			data: { status: "reset_credit", outcome: "reset" },
		});
		expect(shown?.usage?.reset_credits).toBe(1);
		expect(shown?.usage?.windows.map((w) => w.used_percent)).toEqual([0]);
		await calls.useResetCredit("acct-codex");
		expect(await calls.useResetCredit("acct-codex")).toEqual({
			status: "ok",
			data: { status: "reset_credit", outcome: "no_credit" },
		});
		expect(shown?.usage?.reset_credits).toBe(0);
		expect((await calls.useResetCredit("acct-gone")).status).toBe("error");
	});
});

describe("a window's spends", () => {
	it("sends one spend however often it's pressed, then keeps its line", async () => {
		let answer: (result: Result<ActionOutcome>) => void = () => {};
		const sent: string[] = [];
		const spends = resetSpends({
			useResetCredit: (account) => {
				sent.push(account);
				return new Promise((resolve) => {
					answer = resolve;
				});
			},
		});
		const first = spends.spend("acct-codex");
		expect(spends.running("acct-codex")).toBe(true);
		expect(spends.store.getState().spends.get("acct-codex")).toEqual({ state: "running" });
		await spends.spend("acct-codex");
		expect(sent).toEqual(["acct-codex"]);

		answer({ status: "ok", data: { status: "reset_credit", outcome: "reset" } });
		await first;
		expect(spends.running("acct-codex")).toBe(false);
		expect(spends.store.getState().spends.get("acct-codex")).toEqual({
			state: "done",
			line: { text: "Reset used. This account's Codex limits start over now.", tone: "done" },
		});
	});

	it("shows a call that throws as a failed line", async () => {
		const spends = resetSpends({
			useResetCredit: () => Promise.reject(new Error("the window closed")),
		});
		await spends.spend("acct-codex");
		expect(spends.store.getState().spends.get("acct-codex")).toEqual({
			state: "done",
			line: { text: "Couldn't use a reset. Error: the window closed.", tone: "failed" },
		});
	});
});
