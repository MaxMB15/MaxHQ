// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which device does what for PRs: the one holding your GitHub sign-in,
// which merges and approves as you, the ones that could hold it, and the
// one that reads a project's PRs. The app picks a project's device the
// same way for `ListPulls`, so a read goes where the list came from.

import { t } from "../i18n";

import type { DeviceId, ProjectId, PullSummary, Timestamp } from "../bindings";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";
import { isLive } from "../workspaces/state";

/** Where your GitHub sign-in stands: none, on a device that's offline, or ready. */
export type GithubSignIn =
	| { readonly state: "none" }
	| {
			readonly state: "offline" | "ready";
			/** The device holding it. */
			readonly device: DeviceId;
			/** What the sidebar calls that device. */
			readonly name: string;
			/** Your GitHub login. */
			readonly login: string;
	  };

/**
 * Why your sign-in can't merge or approve now, or null when it can: there's
 * none, or the device holding it is offline.
 */
export const signInReason = (signIn: GithubSignIn, act: "merge" | "approve"): string | null => {
	if (signIn.state === "none")
		return t("Sign in to GitHub in Settings to {{act}} here.", { act: act });
	const doing = act === "merge" ? t("Merging") : t("Approving");
	if (signIn.state === "offline")
		return t("{{doing}} needs {{signIn_name}}, which is offline.", {
			doing: doing,
			signIn_name: signIn.name,
		});
	return null;
};

/** The GitHub login a device holds and when it stored it, if it holds one. */
export const githubLoginOn = (
	view: ViewState,
	device: DeviceId
): { readonly login: string; readonly storedAt: Timestamp } | undefined => {
	for (const credential of all(view, "credential")) {
		if (credential.device === device && credential.kind.type === "github_user") {
			return { login: credential.kind.login, storedAt: credential.stored_at };
		}
	}
	return undefined;
};

/** A device that could hold your GitHub sign-in, for the holder picker. */
export type SignInDevice = { readonly device: DeviceId; readonly name: string };

/** Your GitHub sign-in, from the device whose credentials hold it, a live one first. */
export const githubSignIn = (view: ViewState, now: Timestamp): GithubSignIn => {
	const held = all(view, "credential").flatMap((credential) =>
		credential.kind.type === "github_user"
			? [{ device: credential.device, login: credential.kind.login }]
			: []
	);
	const live = held.find((one) => isLive(view, one.device, now));
	const holder = live ?? held[0];
	if (!holder) return { state: "none" };
	return {
		state: live ? "ready" : "offline",
		device: holder.device,
		name: deviceName(view, holder.device),
		login: holder.login,
	};
};

/** The live devices whose `maxd` can hold your GitHub sign-in, by name. */
export const signInDevices = (view: ViewState, now: Timestamp): SignInDevice[] =>
	all(view, "device_link")
		.flatMap((link) =>
			link.device && link.capabilities?.github_user === true && isLive(view, link.device, now)
				? [{ device: link.device, name: deviceName(view, link.device) }]
				: []
		)
		.sort((a, b) => a.name.localeCompare(b.name));

/** Whether a device holds a GitHub App, which its PR reads go through. */
const holdsApp = (view: ViewState, device: DeviceId): boolean =>
	all(view, "credential").some((c) => c.device === device && c.kind.type === "github_app");

/**
 * The device that reads a project's PRs: of those with a checkout of it, a
 * live one first, then one whose `maxd` reads PRs, then one with a GitHub
 * App. Undefined when no device has the project.
 */
export const pullsDevice = (
	view: ViewState,
	project: ProjectId,
	now: Timestamp
): DeviceId | undefined => {
	const devices = new Set(
		all(view, "workspace")
			.filter((w) => w.project === project)
			.map((w) => w.device)
	);
	const rank = (device: DeviceId) =>
		[
			!isLive(view, device, now),
			linkForDevice(view, device)?.capabilities?.pulls !== true,
			!holdsApp(view, device),
		].map(Number);
	return [...devices].sort((a, b) => {
		const [ra, rb] = [rank(a), rank(b)];
		const i = ra.findIndex((v, n) => v !== rb[n]);
		return i >= 0 ? ra[i]! - rb[i]! : a < b ? -1 : a > b ? 1 : 0;
	})[0];
};

/**
 * The device to read one PR from: that of a live agent it's linked to,
 * else the project's.
 */
export const pullDevice = (
	view: ViewState,
	pull: PullSummary,
	project: ProjectId,
	now: Timestamp
): DeviceId | undefined => {
	for (const id of pull.agents) {
		const agent = view.tables.agent.get(id);
		const device = agent && view.tables.workspace.get(agent.workspace)?.device;
		if (device && isLive(view, device, now)) return device;
	}
	return pullsDevice(view, project, now);
};
