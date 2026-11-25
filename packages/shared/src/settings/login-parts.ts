// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the login pages share on the Mac and on the phone: the devices
// ticked to send a login to, how each one took it, what Forget says once
// the device answered, and the line each kind of held login shows.

import { t } from "../i18n";

import { useState } from "react";

import type { DeviceId } from "../bindings";
import { failureOf, scrub } from "../data/failure";
import type { ActionResult } from "../data/source";
import { sentence } from "../status/format";
import type { ViewState } from "../view/replica";

import { type Held, reachText, type Target, usable } from "./logins";

/**
 * The devices ticked: each one that's connected and can take the login,
 * until you untick it. A device that connects later starts ticked too.
 */
export const useTicks = (
	targets: readonly Target[],
	/** The device to tick alone at first, as Sign in again on its error asks. */
	only?: DeviceId
): [ReadonlySet<DeviceId>, (ticked: ReadonlySet<DeviceId>) => void] => {
	const [unticked, setUnticked] = useState<ReadonlySet<DeviceId>>(
		() =>
			new Set(
				only !== undefined
					? targets.filter((t) => t.device !== only).map((t) => t.device)
					: []
			)
	);
	const ticked = new Set(
		targets.filter((t) => usable(t) && !unticked.has(t.device)).map((t) => t.device)
	);
	const change = (next: ReadonlySet<DeviceId>) =>
		setUnticked((before) => {
			const after = new Set(before);
			for (const t of targets) {
				if (!usable(t)) continue;
				if (next.has(t.device)) after.delete(t.device);
				else after.add(t.device);
			}
			return after;
		});
	return [ticked, change];
};

/** How one device took what was sent. */
export type Outcome = {
	readonly device: DeviceId;
	/** Why it didn't, or null when it did. */
	readonly failure: string | null;
};

/**
 * How each of `devices` took a secret, from the results in the same order.
 * A reply that echoes the secret shows `<secret>` instead.
 */
export const outcomesOf = (
	devices: readonly DeviceId[],
	results: readonly ActionResult[],
	secret: string
): Outcome[] =>
	devices.map((device, i) => {
		const result = results[i];
		const reason = result ? failureOf(result) : t("The device didn't answer");
		return { device, failure: reason === null ? null : scrub(reason, secret) };
	});

/** What Forget says once the device answered, or null when it just went. */
export const forgetNote = (
	result: ActionResult
): { readonly text: string; readonly error: boolean } | null => {
	const failure = failureOf(result);
	if (failure) return { text: sentence(failure), error: true };
	const unconfirmed =
		result.status === "ok" && result.data.status === "signed_out" && !result.data.revoked;
	return unconfirmed
		? {
				text: t(
					"Signed out on the device, but OpenAI didn't confirm it ended the session. Disconnect MaxHQ in ChatGPT's settings to be sure."
				),
				error: true,
			}
		: null;
};

/** A setup-token's name and the Mac login it was minted for. */
export const tokenLine = (view: ViewState, token: Held<"claude_token">): string => {
	const login = view.tables.account.get(token.kind.origin);
	return t("{{label}}, for {{value2}}", {
		label: token.kind.label,
		value2: login
			? t("{{label}} on the Mac", { label: login.label })
			: t("a Mac login that's gone"),
	});
};

/** A Codex session and who it's signed in as. */
export const sessionLine = (session: Held<"chat_gpt">): string =>
	session.kind.email ? t("Codex, {{email}}", { email: session.kind.email }) : "Codex";

/** What a device says about its Codex sign-in. */
export const codexStatus = (target: Target, session: Held<"chat_gpt"> | undefined): string =>
	!target.live
		? t("Connect to {{name}} to sign in.", { name: target.name })
		: session
			? t("Signed in{{value1}}", {
					value1: session.kind.email
						? t(" as {{email}}", { email: session.kind.email })
						: "",
				})
			: t("Not signed in");

/** A GitHub App's ID, slug and key, then the repos it reaches. */
export const appLines = (app: Held<"github_app">): readonly [string, string] => [
	t("App {{app_id}}{{value2}}. Key SHA-256 {{value3}}…", {
		app_id: String(app.kind.app_id),
		value2: app.kind.slug ? `, ${app.kind.slug}` : "",
		value3: app.kind.key_sha256.slice(0, 12),
	}),
	reachText(app.kind.repos),
];

/** An APNs key's ID and team. */
export const apnsLine = (key: Held<"apns">): string =>
	t("Key {{key_id}}, team {{team_id}}.", { key_id: key.kind.key_id, team_id: key.kind.team_id });

/** A Firebase key's project and the service account it signs in as. */
export const fcmLine = (key: Held<"fcm">): string =>
	t("Project {{project_id}}, as {{client_email}}.", {
		project_id: key.kind.project_id,
		client_email: key.kind.client_email,
	});
