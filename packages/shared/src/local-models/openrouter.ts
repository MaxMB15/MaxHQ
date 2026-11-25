// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// OpenRouter in Add server, and Sign in with OpenRouter. OpenRouter serves
// every model at one URL, so its choice fills that in. Sign in with
// OpenRouter is a browser round, as Sign in with ChatGPT is: the device's
// maxd starts it and keeps its secret, this Mac's app opens OpenRouter's
// page and waits for the browser to come back, and the device trades the
// code for a key named "MaxHQ on <device>", which never leaves it. The key
// goes to the device's OpenRouter server, which keeps its ID, or to a new
// one. A spending limit in dollars goes along in cents, and OpenRouter
// stops the key there. A device whose maxd is older than
// CLOUD_SERVERS_SINCE can't run a round, so it's offered pasting a key
// only. Only the desktop app runs rounds.

import { t } from "../i18n";

import type { DeviceId, ModelServer, ModelServerId } from "../bindings";
import { failureOf } from "../data/failure";
import type { Ask } from "../data/request-source";
import type { ActionResult, CommandResult } from "../data/source";
import { sentence } from "../status/format";
import type { ViewState } from "../view/replica";

import { serversOf } from "./select";
import { urlParts } from "./wording";

/** Where OpenRouter serves its models, as a server's URL. */
export const OPENROUTER_URL = "https://openrouter.ai/api";

/** OpenRouter's page that makes, lists and revokes keys. */
export const OPENROUTER_KEYS_URL = "https://openrouter.ai/settings/keys";

/** The name an OpenRouter server gets. */
export const OPENROUTER_LABEL = "OpenRouter";

/** The largest spending limit a key takes, in cents, as `maxd` checks it. */
export const MAX_LIMIT_CENTS = 99_999_999;

/** The Sign in with OpenRouter calls of a data source. */
export type OpenRouterCalls = {
	/**
	 * Runs a whole Sign in with OpenRouter round for a device, browser and
	 * callback included. The key goes to the device's OpenRouter server or
	 * a new one, and OpenRouter stops it at `limitCents` when one is given.
	 * Desktop only, to a `maxd` on CLOUD_SERVERS_SINCE or later.
	 */
	readonly signInOpenRouter: (
		device: DeviceId,
		limitCents: number | null
	) => Promise<ActionResult>;
	/** Ends the round that waits for the browser, if one does. Desktop only. */
	readonly cancelSignIn: () => Promise<CommandResult>;
};

/** The Sign in with OpenRouter calls, each one request over `ask`. */
export const openRouterCalls = (ask: Ask): OpenRouterCalls => ({
	signInOpenRouter: (device, limitCents) =>
		ask({ kind: "sign_in_openrouter", device, label: null, limit_cents: limitCents }),
	cancelSignIn: () => ask({ kind: "cancel_sign_in" }),
});

/**
 * Whether `url` is OpenRouter's, on `openrouter.ai` or one of its
 * subdomains over https, as `maxd` checks it.
 */
export const isOpenRouterUrl = (url: string | null | undefined): boolean => {
	const parts = url ? urlParts(url) : null;
	if (parts?.scheme !== "https") return false;
	const host = parts.host.replace(/\.+$/, "");
	return host === "openrouter.ai" || host.endsWith(".openrouter.ai");
};

/** Whether a server is OpenRouter's: a URL you added on OpenRouter's host. */
export const isOpenRouter = (server: Pick<ModelServer, "kind" | "base_url">): boolean =>
	server.kind.type === "external" && isOpenRouterUrl(server.base_url);

/**
 * The device's OpenRouter server, which a sign-in gives a new key, if it
 * has one. The one at OpenRouter's own URL comes first, as `maxd` picks it.
 */
export const openRouterServerOf = (view: ViewState, device: DeviceId): ModelServer | undefined => {
	const servers = serversOf(view, device).filter(isOpenRouter);
	return servers.find((server) => server.base_url === OPENROUTER_URL) ?? servers[0];
};

/** A spending limit as typed: none, an amount in cents, or why it isn't one. */
export type SpendingLimit =
	| { readonly ok: true; readonly cents: number | null }
	| { readonly ok: false; readonly problem: string };

/** Dollars with up to two decimals, after a "." or a ",", and an optional "$". */
const AMOUNT = /^\$?\s*(\d+)(?:[.,](\d{1,2}))?$/;

/** The spending limit typed in dollars, in cents. Empty means no limit. */
export const spendingLimit = (typed: string): SpendingLimit => {
	const text = typed.trim();
	if (!text) return { ok: true, cents: null };
	const match = AMOUNT.exec(text);
	if (!match) {
		return { ok: false, problem: t("Type an amount in dollars, such as 25 or 12.50.") };
	}
	const cents = Number(match[1]) * 100 + Number((match[2] ?? "").padEnd(2, "0"));
	if (cents < 1) {
		return { ok: false, problem: t("Type an amount above 0, or leave it empty for no limit.") };
	}
	if (cents > MAX_LIMIT_CENTS) {
		return { ok: false, problem: t("OpenRouter takes a limit below one million dollars.") };
	}
	return { ok: true, cents };
};

/** Where a Sign in with OpenRouter round is. */
export type OpenRouterRound =
	| { readonly state: "idle" }
	/** Waiting for you to finish in the browser. */
	| { readonly state: "waiting" }
	/** It failed, with why in words. */
	| { readonly state: "failed"; readonly reason: string }
	/** The device keeps the key on this server. */
	| { readonly state: "signed_in"; readonly server: ModelServerId | null };

/**
 * Where a round is once it came back with `result`. One you cancelled goes
 * back to the start, with nothing to say.
 */
export const roundEnded = (result: ActionResult, cancelled: boolean): OpenRouterRound => {
	if (cancelled) return { state: "idle" };
	const failure = failureOf(result);
	if (failure) return { state: "failed", reason: `${sentence(failure)}.` };
	const server =
		result.status === "ok" && result.data.status === "model_server_added"
			? result.data.server
			: null;
	return { state: "signed_in", server };
};

/** What a round that broke before it came back says. */
export const roundBroke = (): OpenRouterRound => ({
	state: "failed",
	reason: t("Couldn't finish the sign-in. Try again."),
});

/** The line under the form while a round waits for the browser. */
export const waitingText = (): string => t("Finish signing in in your browser");

/** What the form says about the device's OpenRouter server, when it has one. */
export const replacesKeyText = (server: Pick<ModelServer, "label">): string =>
	t("Signing in gives {{server}} on this device a new key.", { server: server.label });

/** What a server's row says once Sign in again gave it a new key. */
export const signedInAgainText = (): string =>
	t("Signed in with a new key. You can revoke the old one on OpenRouter's keys page.");

/** Why a device offers pasting a key only. */
export const updateToSignInText = (): string =>
	t("Update this device's maxd to sign in with OpenRouter.");
