// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Signing a Claude, Codex or Cursor login in again from MaxHQ. The device that
// holds the login runs the CLI's own login, and the CLI stores what it gets,
// so MaxHQ never sees it. Here: what Sign in again does for an account,
// where a round stands, how often it asks the device, and whether the app
// opens the page itself. The desktop's dialog and the phone's sheet both
// run a round through `useCliSignIn`.

import { t } from "../i18n";

import { useCallback, useEffect, useLayoutEffect, useReducer, useRef } from "react";

import {
	type Account,
	type AccountId,
	CLI_LOGIN_SINCE,
	CURSOR_LOGIN_SINCE,
	type DeviceId,
	type SignInId,
	type Vendor,
} from "../bindings";
import { failureOf, scrub } from "../data/failure";
import type { ActionResult, DataSource } from "../data/source";
import { sentence } from "../status/format";
import type { ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";

import { isThisMac } from "./app-keys";

/** A section of the desktop's Settings, Logins. */
export type LoginSection = "claude" | "codex" | "cursor";

/**
 * What Sign in again does for an account: the in-app login, a section of
 * Settings, Logins, or nothing, when there's no button.
 */
export type SignInRoute = "login" | { readonly settings: LoginSection } | null;

/**
 * What Sign in again does for `account`. A login in a config folder signs
 * in again in the app. A token, a Codex session or a Cursor key is sent
 * from Settings, Logins. A local ACP command signs in through its own CLI,
 * a local model's account has no login, and an account MaxHQ doesn't know
 * has nothing to sign in.
 */
export const signInRoute = (account: Account | undefined): SignInRoute => {
	if (!account) return null;
	switch (account.login ?? "config_dir") {
		case "config_dir":
			return account.vendor === "acp" ? null : "login";
		case "setup_token":
			return { settings: "claude" };
		case "chat_gpt_sign_in":
			return { settings: "codex" };
		case "cursor_api_key":
			return { settings: "cursor" };
		case "acp_command":
		case "model_server":
			return null;
	}
};

/**
 * Why the app can't sign `account` in again on its device, or null when it
 * can. A `maxd` from before the in-app login for its vendor doesn't run
 * it, and one whose version isn't known yet is taken as older.
 */
export const cliLoginBlocked = (view: ViewState, account: Account): string | null => {
	const since = account.vendor === "cursor" ? CURSOR_LOGIN_SINCE : CLI_LOGIN_SINCE;
	const protocol = linkForDevice(view, account.device)?.protocol ?? null;
	if (protocol !== null && protocol >= since) return null;
	return t("Update maxd on {{deviceName_view__account_device}} to sign in from here.", {
		deviceName_view__account_device: deviceName(view, account.device),
	});
};

/** What Sign in again on an error does. */
export type SignInAction =
	/** Opens the in-app login for the account. */
	| { readonly kind: "login"; readonly account: Account }
	/**
	 * Opens Settings, Logins at a section, for the account's device. `why`
	 * says why the in-app login can't run there, when it can't.
	 */
	| {
			readonly kind: "settings";
			readonly section: LoginSection;
			readonly device: DeviceId;
			readonly why: string | null;
	  };

/** What Sign in again does for the account an error names, or null for no button. */
export const signInAction = (
	view: ViewState,
	id: AccountId | null | undefined
): SignInAction | null => {
	const account = id ? view.tables.account.get(id) : undefined;
	const route = signInRoute(account);
	if (!account || route === null) return null;
	const device = account.device;
	if (route !== "login") return { kind: "settings", section: route.settings, device, why: null };
	const why = cliLoginBlocked(view, account);
	if (why === null) return { kind: "login", account };
	return { kind: "settings", section: sectionOf(account.vendor), device, why };
};

// The section of Settings, Logins for a vendor whose login signs in in the app.
const sectionOf = (vendor: Vendor): LoginSection =>
	vendor === "codex" || vendor === "cursor" ? vendor : "claude";

/** Where a round runs from. */
export type SignInClient = "desktop" | "phone";

// The CLI runs on the account's device. On the Mac the desktop app runs
// on, the browser it opens reaches the CLI on 127.0.0.1.
const onThisMac = (view: ViewState, account: Account, client: SignInClient): boolean =>
	client === "desktop" && isThisMac(view, account.device);

/**
 * Whether to ask Codex for a code to enter on any device, in place of a
 * page that comes back to the device. Only the desktop app's own Mac can
 * take that page back, so the phone always asks.
 */
export const wantsDeviceCode = (view: ViewState, account: Account, client: SignInClient): boolean =>
	account.vendor === "codex" && !onThisMac(view, account, client);

/**
 * Whether the app opens the sign-in page itself. On the desktop app's own
 * Mac the CLI opened the browser already, all but Cursor's, which the
 * device runs without one.
 */
export const opensPageItself = (view: ViewState, account: Account, client: SignInClient): boolean =>
	account.vendor === "cursor" || !onThisMac(view, account, client);

const TITLES: Readonly<Partial<Record<Vendor, string>>> = {
	get codex() {
		return t("Sign in to Codex");
	},
	get cursor() {
		return t("Sign in to Cursor");
	},
};

/** The dialog's title for a vendor's login. */
export const signInTitle = (vendor: Vendor): string =>
	TITLES[vendor] ?? t("Sign in to Claude Code");

/** What a round that went through says, with the account as it is now. */
export const signedInText = (account: Account | undefined): string =>
	account?.email
		? t("Signed in as {{account_email}}.", { account_email: account.email })
		: t("Signed in.");

/** What a round that went through says next. */
export const SEND_AGAIN = "Send your message again to continue.";

/**
 * Whether the account's newest usage check found it logged out. A good
 * check since then means it signed in again.
 */
export const loggedOut = (account: Account): boolean => {
	const failure = account.usage_failure;
	if (!failure || (account.usage && failure.at <= account.usage.checked_at)) return false;
	return /\blogged out\b/i.test(failure.reason);
};

// The sites each vendor's sign-in pages are on. The device names the page
// and the app opens it, so it has to be one of these.
const SITES: Readonly<Record<Vendor, readonly string[]>> = {
	claude: ["claude.com", "claude.ai", "anthropic.com"],
	codex: ["openai.com", "chatgpt.com"],
	cursor: ["cursor.com"],
	acp: [],
};

// A URL with no user part, so the host is the part after `https://`.
// Parsed by hand, since the phone's URL can't read a host.
const HTTPS_HOST = /^https:\/\/([a-z0-9.-]+)(?::\d+)?(?:[/?#]|$)/i;

/** Whether `url` is a page on the vendor's own site, over HTTPS. */
export const onVendorSite = (url: string, vendor: Vendor): boolean => {
	const host = HTTPS_HOST.exec(url)?.[1]?.toLowerCase();
	if (!host) return false;
	return SITES[vendor].some((site) => host === site || host.endsWith(`.${site}`));
};

/** The page a waiting round signs in on. */
export type SignInPage = {
	readonly signIn: SignInId;
	readonly url: string;
	/** The code to enter on the page, for a Codex device code sign-in. */
	readonly userCode: string | null;
	/** Whether the page shows a code to paste back, as Claude's does. */
	readonly paste: boolean;
	/** Whether the app opened the page, rather than the CLI. */
	readonly opened: boolean;
};

/** Where a round stands. */
export type CliSignIn =
	| { readonly step: "starting" }
	/** Waiting for you to sign in on the page. */
	| ({ readonly step: "waiting" } & SignInPage)
	/** The code you pasted went to the CLI, which checks it. */
	| ({ readonly step: "checking" } & SignInPage)
	| { readonly step: "signed_in" }
	| { readonly step: "failed"; readonly message: string }
	| { readonly step: "cancelled" };

/** What happens to a round. */
export type CliSignInEvent =
	/** A new Begin went out. */
	| { readonly type: "begin" }
	/** Begin's answer. `opens` says whether the app opens the page. */
	| {
			readonly type: "begun";
			readonly result: ActionResult;
			readonly vendor: Vendor;
			readonly opens: boolean;
	  }
	/** The pasted code went out. */
	| { readonly type: "finish" }
	/**
	 * A Check's or Finish's answer for a sign-in. `code` is the code that
	 * Finish sent, so a reply that echoes it never shows it.
	 */
	| {
			readonly type: "answer";
			readonly from: "check" | "finish";
			readonly signIn: SignInId;
			readonly result: ActionResult;
			readonly code?: string;
	  }
	/** You closed it while it waited. */
	| { readonly type: "cancel" };

/** A round that just sent Begin. */
export const STARTING: CliSignIn = { step: "starting" };

const failed = (message: string): CliSignIn => ({
	step: "failed",
	message: `${sentence(message)}.`,
});

const pageOf = (result: ActionResult, vendor: Vendor, opens: boolean): CliSignIn => {
	const failure = failureOf(result);
	if (failure !== null) return failed(failure);
	if (result.status !== "ok" || result.data.status !== "cli_login") {
		return failed(t("The device didn't give a sign-in page"));
	}
	const { sign_in, url, user_code, paste } = result.data;
	if (!onVendorSite(url, vendor)) {
		return failed(
			t("The device named a sign-in page MaxHQ doesn't know, so it didn't open it")
		);
	}
	return { step: "waiting", signIn: sign_in, url, userCode: user_code, paste, opened: opens };
};

const isWaiting = (state: CliSignIn): state is Extract<CliSignIn, { signIn: SignInId }> =>
	state.step === "waiting" || state.step === "checking";

/** The next state of a round. */
export const cliSignIn = (state: CliSignIn, event: CliSignInEvent): CliSignIn => {
	switch (event.type) {
		case "begin":
			return STARTING;
		case "begun":
			return state.step === "starting"
				? pageOf(event.result, event.vendor, event.opens)
				: state;
		case "finish":
			return state.step === "waiting" ? { ...state, step: "checking" } : state;
		case "answer": {
			// An answer for a round you closed or began again comes too late.
			if (!isWaiting(state) || state.signIn !== event.signIn) return state;
			const { result } = event;
			// A Check that didn't get through tells nothing, and the next one asks again.
			if (result.status === "error" && event.from === "check") return state;
			const failure = failureOf(result);
			if (failure !== null) return failed(scrub(failure, event.code ?? ""));
			if (result.status !== "ok" || result.data.status !== "cli_login_state") return state;
			const now = result.data.state;
			if (now.state === "signed_in") return { step: "signed_in" };
			if (now.state === "failed") return failed(scrub(now.message, event.code ?? ""));
			return state;
		}
		case "cancel":
			return state.step === "starting" || isWaiting(state) ? { step: "cancelled" } : state;
	}
};

/** How often a waiting round asks its device how the login stands. */
export const CHECK_EVERY_MS = 2000;

/**
 * Sends `check` every `every` ms, one at a time, and hands each answer to
 * `answer`. A send that throws counts as one that didn't get through.
 * Returns what stops it.
 */
export const pollCliLogin = (
	check: () => Promise<ActionResult>,
	answer: (result: ActionResult) => void,
	every: number = CHECK_EVERY_MS
): (() => void) => {
	let stopped = false;
	let timer: ReturnType<typeof setTimeout> | undefined;
	const next = () => {
		timer = setTimeout(() => {
			void settle(check()).then((result) => {
				if (stopped) return;
				answer(result);
				next();
			});
		}, every);
	};
	next();
	return () => {
		stopped = true;
		clearTimeout(timer);
	};
};

const settle = (pending: Promise<ActionResult>): Promise<ActionResult> =>
	pending.catch((error: unknown): ActionResult => ({
		status: "error",
		error: { kind: "failed", message: error instanceof Error ? error.message : String(error) },
	}));

/** The calls a round makes. */
export type CliLoginCalls = Pick<
	DataSource,
	"beginCliLogin" | "finishCliLogin" | "checkCliLogin" | "cancelCliLogin"
>;

/** How a round runs, from `wantsDeviceCode` and `opensPageItself`. */
export type SignInOptions = {
	readonly deviceCode: boolean;
	readonly opens: boolean;
	/** Opens a page in the browser, the platform's own way. */
	readonly open: (url: string) => void;
};

/** A round, and what you can do to it. */
export type CliSignInRound = {
	readonly state: CliSignIn;
	/** Begins again, for Try again. */
	readonly begin: () => void;
	/** Sends the code you pasted. */
	readonly finish: (code: string) => void;
	/** Stops a round that waits, and has the device stop the CLI. */
	readonly cancel: () => void;
};

/**
 * Runs a round for `account` while the caller is mounted: Begin at once,
 * Check every 2 s while it waits, and Cancel when the caller goes away
 * while it still waits.
 */
export const useCliSignIn = (
	calls: CliLoginCalls,
	account: Account,
	options: SignInOptions
): CliSignInRound => {
	const [state, dispatch] = useReducer(cliSignIn, STARTING);
	const { device, id, vendor } = account;
	const latest = useRef({ calls, options });
	useLayoutEffect(() => {
		latest.current = { calls, options };
	});
	// Each Begin and each Cancel starts a new round, so an answer for an
	// older one is dropped, and a sign-in it started stops on the device.
	const round = useRef(0);
	// The sign-in the device waits on, for Cancel.
	const live = useRef<SignInId | null>(null);

	const stopLive = useCallback(() => {
		const signIn = live.current;
		live.current = null;
		if (signIn !== null) void settle(latest.current.calls.cancelCliLogin(device, signIn));
	}, [device]);

	const begin = useCallback(() => {
		const mine = ++round.current;
		stopLive();
		const { calls, options } = latest.current;
		dispatch({ type: "begin" });
		void settle(calls.beginCliLogin(device, id, options.deviceCode)).then((result) => {
			const begun = { type: "begun", result, vendor, opens: options.opens } as const;
			const next = cliSignIn(STARTING, begun);
			if (round.current !== mine) {
				if (next.step === "waiting") void settle(calls.cancelCliLogin(device, next.signIn));
				return;
			}
			if (next.step === "waiting") {
				live.current = next.signIn;
				if (next.opened) options.open(next.url);
			}
			dispatch(begun);
		});
	}, [device, id, vendor, stopLive]);

	// Drops the round, and has the device stop a sign-in that waits.
	const drop = useCallback(() => {
		round.current += 1;
		stopLive();
	}, [stopLive]);

	const cancel = useCallback(() => {
		drop();
		dispatch({ type: "cancel" });
	}, [drop]);

	const finish = useCallback(
		(typed: string) => {
			const signIn = live.current;
			// As the backend sends it, so a reply that echoes it is scrubbed.
			const code = typed.replace(/\s+/g, "");
			if (signIn === null || code === "") return;
			dispatch({ type: "finish" });
			void settle(latest.current.calls.finishCliLogin(device, signIn, code)).then((result) =>
				dispatch({ type: "answer", from: "finish", signIn, result, code })
			);
		},
		[device]
	);

	// Begin when it opens, a moment later, so a mount React undoes at once
	// in development sends nothing. Closing it while it waits stops the CLI.
	useEffect(() => {
		const timer = setTimeout(begin, 0);
		return () => {
			clearTimeout(timer);
			drop();
		};
	}, [begin, drop]);

	const waitingOn = isWaiting(state) ? state.signIn : null;
	useEffect(() => {
		if (waitingOn === null) {
			live.current = null;
			return;
		}
		return pollCliLogin(
			() => latest.current.calls.checkCliLogin(device, waitingOn),
			(result) => dispatch({ type: "answer", from: "check", signIn: waitingOn, result })
		);
	}, [device, waitingOn]);

	return { state, begin, finish, cancel };
};
