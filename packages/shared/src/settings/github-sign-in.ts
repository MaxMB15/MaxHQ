// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Signing in to GitHub as you on one device, for merges and approvals.
// The device asks GitHub for a code through its App's device flow, you
// enter it on GitHub, and the sign-in is done once that device's
// `github_user` login shows up on the state stream. The Mac and the phone
// both run it from here and only draw it their own way.

import { useEffect, useState } from "react";

import type { DeviceId, SignInId, Timestamp } from "../bindings";
import type { ActionResult, DataSource } from "../data/source";
import { t } from "../i18n";
import { useNow } from "../lib/clock";
import { githubLoginOn, type SignInDevice } from "../pulls/devices";
import { errorText } from "../timeline/store";
import type { ViewState } from "../view/replica";
import { heldOf } from "./logins";

const GITHUB_PAGES = "https://github.com/";

/** Where GitHub lists the Apps you authorized, to revoke one there. */
export const AUTHORIZED_APPS_URL = "https://github.com/settings/apps/authorizations";

/** A GitHub App's settings page, where Enable Device Flow is ticked. */
export const appSettingsUrl = (slug: string): string => `https://github.com/settings/apps/${slug}`;

/** The slug of the GitHub App a device holds, if it holds one GitHub named. */
export const appSlugOn = (view: ViewState, device: DeviceId): string | null =>
	heldOf(view, "github_app").find((c) => c.device === device)?.kind.slug ?? null;

/**
 * The device the picker starts on: the first that holds a GitHub App, since
 * it signs in through its own, else the first there is.
 */
export const defaultHolder = (
	view: ViewState,
	devices: readonly SignInDevice[]
): DeviceId | undefined =>
	(
		devices.find((d) => heldOf(view, "github_app").some((c) => c.device === d.device)) ??
		devices[0]
	)?.device;

/** A code waiting to be entered on GitHub. */
type Waiting = {
	readonly device: DeviceId;
	readonly signIn: SignInId;
	readonly userCode: string;
	readonly verificationUri: string;
	readonly expiresAt: Timestamp;
	/** When the device stored the login it held before, if it held one. */
	readonly before: Timestamp | null;
};

/** Where a GitHub sign-in stands, for the page. */
export type GithubSignInState =
	| { readonly state: "idle" }
	| { readonly state: "starting"; readonly device: DeviceId }
	/** Waiting for you to enter `userCode` at `verificationUri`. */
	| {
			readonly state: "waiting";
			readonly device: DeviceId;
			readonly userCode: string;
			readonly verificationUri: string;
			readonly expiresAt: Timestamp;
	  }
	/** The device holds your sign-in now. */
	| { readonly state: "done"; readonly device: DeviceId; readonly login: string }
	/** The code ran out with no sign-in. Begin again to retry. */
	| { readonly state: "expired"; readonly device: DeviceId }
	/** The device wouldn't start one, or GitHub turned it down. */
	| { readonly state: "failed"; readonly device: DeviceId; readonly reason: string };

/** What the hook keeps between the device's answers. */
export type Started =
	| { readonly state: "idle" }
	| { readonly state: "starting"; readonly device: DeviceId }
	| { readonly state: "waiting"; readonly code: Waiting }
	| { readonly state: "failed"; readonly device: DeviceId; readonly reason: string };

/** How often a waiting sign-in asks its device whether GitHub refused it. */
export const SIGN_IN_CHECK_MS = 2000;

/**
 * What the device's answer to a begin means: a code to enter, or why
 * not. `before` is when the device stored the login it held already.
 */
export const startedFrom = (
	result: ActionResult,
	device: DeviceId,
	before: Timestamp | null
): Started => {
	if (result.status === "error") {
		return { state: "failed", device, reason: errorText(result.error) };
	}
	if (result.data.status === "device_code") {
		const code = result.data;
		// The device names this page and the app opens it for you, so it has to be GitHub's.
		if (!code.verification_uri.startsWith(GITHUB_PAGES)) {
			return {
				state: "failed",
				device,
				reason: t("The device named a sign-in page that isn't on GitHub."),
			};
		}
		return {
			state: "waiting",
			code: {
				device,
				signIn: code.sign_in,
				userCode: code.user_code,
				verificationUri: code.verification_uri,
				expiresAt: code.expires_at,
				before,
			},
		};
	}
	const reason =
		result.data.status === "refused" ? result.data.reason : t("The device didn't give a code.");
	return { state: "failed", device, reason };
};

/**
 * Where a sign-in stands at `now`: done once the device stored a new
 * login, expired once the code's time ran out, else as it started.
 */
export const signInStateOf = (
	started: Started,
	view: ViewState,
	now: number
): GithubSignInState => {
	if (started.state !== "waiting") return started;
	const { device, userCode, verificationUri, expiresAt, before } = started.code;
	const held = githubLoginOn(view, device);
	if (held && held.storedAt !== before) return { state: "done", device, login: held.login };
	if (now >= expiresAt) return { state: "expired", device };
	return { state: "waiting", device, userCode, verificationUri, expiresAt };
};

/** The calls a GitHub sign-in makes. */
export type GithubSignInSource = Pick<
	DataSource,
	"beginGithubSignIn" | "checkGithubSignIn" | "cancelGithubSignIn"
>;

/**
 * Signs you in to GitHub on a device you pick. `begin` asks it for a code
 * to enter on GitHub. While it waits, the device is asked now and then,
 * so a code GitHub turned down says why. A device too old to answer says
 * nothing, and its code shows as expired once its time runs out.
 */
export const useGithubSignIn = (source: GithubSignInSource, view: ViewState) => {
	const [started, setStarted] = useState<Started>({ state: "idle" });
	const now = useNow(1000);

	const begin = async (device: DeviceId) => {
		const before = githubLoginOn(view, device)?.storedAt ?? null;
		setStarted({ state: "starting", device });
		setStarted(startedFrom(await source.beginGithubSignIn(device, null), device, before));
	};

	const waitingOn = started.state === "waiting" ? started.code : null;
	useEffect(() => {
		if (!waitingOn) return;
		const { device, signIn } = waitingOn;
		const timer = setInterval(() => {
			void source.checkGithubSignIn(device, signIn).then((result) => {
				if (result.status !== "ok" || result.data.status !== "refused") return;
				const reason = result.data.reason;
				setStarted((now) =>
					now.state === "waiting" && now.code.signIn === signIn
						? { state: "failed", device, reason }
						: now
				);
			});
		}, SIGN_IN_CHECK_MS);
		return () => clearInterval(timer);
	}, [source, waitingOn]);

	/** Stops waiting for the code, and has the device forget it. */
	const cancel = async () => {
		if (started.state === "waiting") {
			await source.cancelGithubSignIn(started.code.device, started.code.signIn);
		}
		setStarted({ state: "idle" });
	};

	return { state: signInStateOf(started, view, now), begin, cancel };
};
