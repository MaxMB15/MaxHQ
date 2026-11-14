// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { type Account, CLI_LOGIN_SINCE, CURSOR_LOGIN_SINCE } from "../bindings";
import {
	CLI_SIGN_IN_MS,
	MOCK_CLAUDE_PAGE,
	MOCK_CODEX_CODE,
	MOCK_CODEX_DEVICE_PAGE,
	MOCK_CURSOR_PAGE,
	mockCliLogins,
	noCliLogins,
} from "../data/mock-cli-login";
import type { ActionResult } from "../data/source";
import { account, device, entities, link, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	CHECK_EVERY_MS,
	type CliSignIn,
	cliLoginBlocked,
	cliSignIn,
	loggedOut,
	onVendorSite,
	opensPageItself,
	pollCliLogin,
	signedInText,
	signInAction,
	signInRoute,
	signInTitle,
	STARTING,
	wantsDeviceCode,
} from "./cli-sign-in";

const CLAUDE = account({ id: "claude-mac", device: "mac" });
const CODEX = account({ id: "codex-mac", device: "mac", vendor: "codex", label: "Codex" });
const CODEX_DEVBOX = account({ id: "codex-devbox", device: "devbox", vendor: "codex" });
const TOKEN = account({ id: "token", device: "devbox", login: "setup_token" });
const CURSOR = account({ id: "cursor-mac", device: "mac", vendor: "cursor", label: "Cursor" });

const view = (protocol: number | null = CLI_LOGIN_SINCE, accounts: Account[] = []) =>
	applySnapshot({
		seq: 1,
		entities: entities({
			devices: [device({ id: "mac", name: "Mac", os: "macos" }), device({ id: "devbox" })],
			links: [
				link({
					id: "local",
					name: "Mac",
					device: "mac",
					route: { type: "local" },
					protocol,
				}),
				link({ id: "devbox", device: "devbox", protocol }),
			],
			accounts: [CLAUDE, CODEX, CODEX_DEVBOX, TOKEN, CURSOR, ...accounts],
		}),
	});

const ok = (data: Extract<ActionResult, { status: "ok" }>["data"]): ActionResult => ({
	status: "ok",
	data,
});

const page = (
	over: { sign_in?: string; url?: string; user_code?: string | null; paste?: boolean } = {}
) =>
	ok({
		status: "cli_login",
		sign_in: over.sign_in ?? "s-1",
		url: over.url ?? MOCK_CLAUDE_PAGE,
		user_code: over.user_code ?? null,
		paste: over.paste ?? true,
	});

const stateOf = (state: "waiting" | "signed_in"): ActionResult =>
	ok({ status: "cli_login_state", state: { state } });

const waiting = (): CliSignIn =>
	cliSignIn(STARTING, { type: "begun", result: page(), vendor: "claude", opens: true });

describe("sign in again", () => {
	it("routes each kind of login", () => {
		expect(signInRoute(CLAUDE)).toBe("login");
		expect(signInRoute(CODEX)).toBe("login");
		expect(signInRoute(CURSOR)).toBe("login");
		expect(signInRoute(TOKEN)).toEqual({ settings: "claude" });
		expect(
			signInRoute(account({ id: "g", vendor: "codex", login: "chat_gpt_sign_in" }))
		).toEqual({ settings: "codex" });
		expect(
			signInRoute(account({ id: "k", vendor: "cursor", login: "cursor_api_key" }))
		).toEqual({
			settings: "cursor",
		});
		expect(signInRoute(account({ id: "a", vendor: "acp", login: "acp_command" }))).toBeNull();
		expect(signInRoute(undefined)).toBeNull();
	});

	it("opens the login, or Settings when the device's maxd is older", () => {
		expect(signInAction(view(), "claude-mac")).toMatchObject({
			kind: "login",
			account: CLAUDE,
		});
		expect(signInAction(view(), "token")).toEqual({
			kind: "settings",
			section: "claude",
			device: "devbox",
			why: null,
		});
		expect(signInAction(view(CLI_LOGIN_SINCE - 1), "codex-devbox")).toEqual({
			kind: "settings",
			section: "codex",
			device: "devbox",
			why: "Update maxd on devbox to sign in from here.",
		});
		expect(signInAction(view(CURSOR_LOGIN_SINCE), "cursor-mac")).toMatchObject({
			kind: "login",
			account: CURSOR,
		});
		expect(signInAction(view(), "gone")).toBeNull();
		expect(signInAction(view(), null)).toBeNull();
	});

	it("takes a device whose version isn't known yet as older", () => {
		expect(cliLoginBlocked(view(null), CLAUDE)).toBe(
			"Update maxd on Mac to sign in from here."
		);
		expect(cliLoginBlocked(view(CLI_LOGIN_SINCE + 1), CLAUDE)).toBeNull();
	});

	it("signs Cursor in only on a maxd that runs its login", () => {
		const older = view(CURSOR_LOGIN_SINCE - 1);
		expect(cliLoginBlocked(older, CODEX)).toBeNull();
		expect(cliLoginBlocked(older, CURSOR)).toBe("Update maxd on Mac to sign in from here.");
		expect(signInAction(older, "cursor-mac")).toEqual({
			kind: "settings",
			section: "cursor",
			device: "mac",
			why: "Update maxd on Mac to sign in from here.",
		});
	});

	it("asks Codex for a device code everywhere but on the app's own Mac", () => {
		const v = view();
		expect(wantsDeviceCode(v, CODEX, "desktop")).toBe(false);
		expect(wantsDeviceCode(v, CODEX, "phone")).toBe(true);
		expect(wantsDeviceCode(v, CODEX_DEVBOX, "desktop")).toBe(true);
		expect(wantsDeviceCode(v, CLAUDE, "phone")).toBe(false);
		expect(wantsDeviceCode(v, CURSOR, "phone")).toBe(false);
	});

	it("opens the page itself unless the CLI runs on the app's own Mac", () => {
		const v = view();
		expect(opensPageItself(v, CLAUDE, "desktop")).toBe(false);
		expect(opensPageItself(v, CLAUDE, "phone")).toBe(true);
		expect(opensPageItself(v, CODEX_DEVBOX, "desktop")).toBe(true);
		// Cursor's CLI runs without a browser, on this Mac too.
		expect(opensPageItself(v, CURSOR, "desktop")).toBe(true);
	});

	it("only takes a page on the vendor's own site", () => {
		expect(onVendorSite(MOCK_CLAUDE_PAGE, "claude")).toBe(true);
		expect(onVendorSite("https://console.anthropic.com/oauth", "claude")).toBe(true);
		expect(onVendorSite(MOCK_CODEX_DEVICE_PAGE, "codex")).toBe(true);
		expect(onVendorSite("https://AUTH.OPENAI.COM:443/x", "codex")).toBe(true);
		expect(onVendorSite(MOCK_CODEX_DEVICE_PAGE, "claude")).toBe(false);
		expect(onVendorSite("http://claude.com/x", "claude")).toBe(false);
		expect(onVendorSite("https://claude.com.example.net/x", "claude")).toBe(false);
		expect(onVendorSite("https://evilclaude.com/x", "claude")).toBe(false);
		expect(onVendorSite("https://user@claude.com/x", "claude")).toBe(false);
		expect(onVendorSite("https://claude.com/x", "cursor")).toBe(false);
		expect(onVendorSite(MOCK_CURSOR_PAGE, "cursor")).toBe(true);
		expect(onVendorSite(MOCK_CURSOR_PAGE, "acp")).toBe(false);
	});

	it("titles and the words once signed in", () => {
		expect(signInTitle("claude")).toBe("Sign in to Claude Code");
		expect(signInTitle("codex")).toBe("Sign in to Codex");
		expect(signInTitle("cursor")).toBe("Sign in to Cursor");
		expect(signedInText(CLAUDE)).toBe("Signed in as someone@example.com.");
		expect(signedInText({ ...CLAUDE, email: null })).toBe("Signed in.");
	});

	it("sees a login the newest usage check found logged out", () => {
		const failure = { at: NOW, reason: "logged out" };
		expect(loggedOut({ ...CLAUDE, usage_failure: failure })).toBe(true);
		expect(loggedOut({ ...CLAUDE, usage_failure: { at: NOW, reason: "timed out" } })).toBe(
			false
		);
		expect(loggedOut(CLAUDE)).toBe(false);
		const usage = { checked_at: NOW + 1 } as NonNullable<Account["usage"]>;
		expect(loggedOut({ ...CLAUDE, usage, usage_failure: failure })).toBe(false);
	});
});

describe("a round", () => {
	it("waits on the page Begin gave", () => {
		expect(waiting()).toEqual({
			step: "waiting",
			signIn: "s-1",
			url: MOCK_CLAUDE_PAGE,
			userCode: null,
			paste: true,
			opened: true,
		});
	});

	it("fails when Begin is refused or names a page elsewhere", () => {
		const refused = ok({ status: "refused", reason: "claude isn't installed" });
		expect(
			cliSignIn(STARTING, { type: "begun", result: refused, vendor: "claude", opens: true })
		).toEqual({ step: "failed", message: "Claude isn't installed." });
		const elsewhere = page({ url: "https://example.net/login" });
		expect(
			cliSignIn(STARTING, { type: "begun", result: elsewhere, vendor: "claude", opens: true })
		).toEqual({
			step: "failed",
			message: "The device named a sign-in page MaxHQ doesn't know, so it didn't open it.",
		});
	});

	it("checks the code once it's sent and signs in", () => {
		const checking = cliSignIn(waiting(), { type: "finish" });
		expect(checking.step).toBe("checking");
		const done = cliSignIn(checking, {
			type: "answer",
			from: "finish",
			signIn: "s-1",
			result: stateOf("signed_in"),
			code: "abc",
		});
		expect(done).toEqual({ step: "signed_in" });
	});

	it("never shows the code a failure echoes", () => {
		const failed = cliSignIn(cliSignIn(waiting(), { type: "finish" }), {
			type: "answer",
			from: "finish",
			signIn: "s-1",
			result: ok({
				status: "cli_login_state",
				state: { state: "failed", message: "code secret-code was wrong" },
			}),
			code: "secret-code",
		});
		expect(failed).toEqual({ step: "failed", message: "Code <secret> was wrong." });
	});

	it("drops answers for another sign-in and Checks that didn't get through", () => {
		const w = waiting();
		const other = { type: "answer", from: "check", signIn: "s-2" } as const;
		expect(cliSignIn(w, { ...other, result: stateOf("signed_in") })).toBe(w);
		const lost: ActionResult = {
			status: "error",
			error: { kind: "not_connected", device: "mac" },
		};
		expect(cliSignIn(w, { ...other, signIn: "s-1", result: lost })).toBe(w);
		expect(cliSignIn(w, { ...other, signIn: "s-1", result: stateOf("waiting") })).toBe(w);
	});

	it("cancels only while it runs, and Begin starts over", () => {
		expect(cliSignIn(waiting(), { type: "cancel" })).toEqual({ step: "cancelled" });
		expect(cliSignIn(STARTING, { type: "cancel" })).toEqual({ step: "cancelled" });
		const done = { step: "signed_in" } as const;
		expect(cliSignIn(done, { type: "cancel" })).toBe(done);
		expect(cliSignIn({ step: "failed", message: "x" }, { type: "begin" })).toBe(STARTING);
	});
});

describe("the check poll", () => {
	afterEach(() => vi.useRealTimers());

	it("asks every 2 s, one at a time, until stopped", async () => {
		vi.useFakeTimers();
		const check = vi.fn(async () => stateOf("waiting"));
		const answers: ActionResult[] = [];
		const stop = pollCliLogin(check, (r) => answers.push(r));
		await vi.advanceTimersByTimeAsync(CHECK_EVERY_MS - 1);
		expect(check).not.toHaveBeenCalled();
		await vi.advanceTimersByTimeAsync(1);
		expect(check).toHaveBeenCalledTimes(1);
		await vi.advanceTimersByTimeAsync(CHECK_EVERY_MS);
		expect(answers).toHaveLength(2);
		stop();
		await vi.advanceTimersByTimeAsync(CHECK_EVERY_MS * 3);
		expect(check).toHaveBeenCalledTimes(2);
	});

	it("takes a Check that throws as one that didn't get through", async () => {
		vi.useFakeTimers();
		const answers: ActionResult[] = [];
		const stop = pollCliLogin(
			() => Promise.reject(new Error("socket closed")),
			(r) => answers.push(r)
		);
		await vi.advanceTimersByTimeAsync(CHECK_EVERY_MS);
		expect(answers).toEqual([
			{ status: "error", error: { kind: "failed", message: "socket closed" } },
		]);
		stop();
	});
});

describe("the sample logins", () => {
	afterEach(() => vi.useRealTimers());

	const sample = () => {
		const signedIn: string[] = [];
		const accounts = new Map([CLAUDE, CODEX, CURSOR].map((a) => [a.id, a]));
		const { calls } = mockCliLogins({
			accountOf: (id) => accounts.get(id),
			signedIn: (id) => signedIn.push(id),
		});
		return { calls, signedIn };
	};

	it("takes a pasted Claude code", async () => {
		const { calls, signedIn } = sample();
		const begun = await calls.beginCliLogin("mac", CLAUDE.id, false);
		expect(begun).toEqual(page({ sign_in: "cli-sign-in-1" }));
		expect(await calls.finishCliLogin("mac", "cli-sign-in-1", "abc")).toEqual(
			stateOf("signed_in")
		);
		expect(signedIn).toEqual([CLAUDE.id]);
	});

	it("gives Codex a code and signs in by itself", async () => {
		vi.useFakeTimers();
		const { calls, signedIn } = sample();
		const begun = await calls.beginCliLogin("mac", CODEX.id, true);
		expect(begun).toEqual(
			ok({
				status: "cli_login",
				sign_in: "cli-sign-in-1",
				url: MOCK_CODEX_DEVICE_PAGE,
				user_code: MOCK_CODEX_CODE,
				paste: false,
			})
		);
		expect(await calls.checkCliLogin("mac", "cli-sign-in-1")).toEqual(stateOf("waiting"));
		await vi.advanceTimersByTimeAsync(CLI_SIGN_IN_MS);
		expect(await calls.checkCliLogin("mac", "cli-sign-in-1")).toEqual(stateOf("signed_in"));
		expect(signedIn).toEqual([CODEX.id]);
	});

	it("gives Cursor a page, never a code, and signs in by itself", async () => {
		vi.useFakeTimers();
		const { calls, signedIn } = sample();
		const begun = await calls.beginCliLogin("mac", CURSOR.id, true);
		expect(begun).toEqual(
			page({ sign_in: "cli-sign-in-1", url: MOCK_CURSOR_PAGE, paste: false })
		);
		await vi.advanceTimersByTimeAsync(CLI_SIGN_IN_MS);
		expect(await calls.checkCliLogin("mac", "cli-sign-in-1")).toEqual(stateOf("signed_in"));
		expect(signedIn).toEqual([CURSOR.id]);
	});

	it("stops a Codex login you cancel", async () => {
		vi.useFakeTimers();
		const { calls, signedIn } = sample();
		await calls.beginCliLogin("mac", CODEX.id, false);
		await calls.cancelCliLogin("mac", "cli-sign-in-1");
		await vi.advanceTimersByTimeAsync(CLI_SIGN_IN_MS);
		expect(signedIn).toEqual([]);
	});

	it("refuses where no login runs", async () => {
		expect(await noCliLogins.beginCliLogin("mac", CLAUDE.id, false)).toEqual(
			ok({ status: "refused", reason: "This device can't sign in again from MaxHQ." })
		);
	});
});
