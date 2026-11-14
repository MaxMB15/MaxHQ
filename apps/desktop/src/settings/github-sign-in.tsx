// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Merge and approve as you: one device signs in to GitHub through its App's
// device flow and holds the sign-in, and merges and approvals go through
// it. Signed out, you pick that device, a live one holding a GitHub App
// first, since it signs in through its own App. Signing in shows the code
// to enter, opens GitHub's page for it in the browser, and waits until the
// device holds your login. A code that runs out says what to tick in the
// App's settings, since GitHub refuses one when the App's device flow is
// off. Signed in, it says where, and Sign out has that device forget it.

import { useEffect, useRef, useState } from "react";

import { useView } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import { CopyIcon } from "@/components/copy-field";
import { useGithubSignIn } from "@/pulls/queries";
import type { DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import {
	type GithubSignIn,
	githubSignIn,
	type SignInDevice,
	signInDevices,
} from "@maxhq/shared/pulls/devices";
import { sentence } from "@maxhq/shared/status/format";
import type { ViewState } from "@maxhq/shared/view/replica";
import { deviceName } from "@maxhq/shared/view/select";

import { PRIMARY, Row, SECONDARY, Section, StatusNote } from "./controls";
import { type ForgetWords, Forget } from "./login-parts";
import { heldOf } from "./logins";
import { Pick } from "./snooze-presets";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** Where GitHub lists the Apps you authorized, to revoke one there. */
const AUTHORIZED_APPS_URL = "https://github.com/settings/apps/authorizations";

/** A GitHub App's settings page, where Enable Device Flow is ticked. */
export const appSettingsUrl = (slug: string): string => `https://github.com/settings/apps/${slug}`;

const SIGN_OUT: ForgetWords = {
	get label() {
		return translate("Sign out");
	},
	get busy() {
		return translate("Signing out");
	},
};

const LINK = "text-xs text-primary hover:underline";

/** The slug of the GitHub App a device holds, if it holds one GitHub named. */
const appSlugOn = (view: ViewState, device: DeviceId): string | null =>
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

const SignedIn = ({ signIn }: { signIn: Exclude<GithubSignIn, { state: "none" }> }) => {
	useI18n();
	const view = useView((s) => s.view);
	const credential = heldOf(view, "github_user").find((c) => c.device === signIn.device);
	const offline = signIn.state === "offline";
	return (
		<Row
			title={translate("Signed in as @{{login}} on {{name}}{{value3}}", {
				login: signIn.login,
				name: signIn.name,
				value3: offline ? translate(", which is offline") : "",
			})}
			sub={
				<>
					{translate("Merges and approvals go through ")}
					{signIn.name}{" "}
					{translate(
						" as you. Sign out deletes the sign-in there. To revoke it on GitHub too, use"
					)}{" "}
					<a href={AUTHORIZED_APPS_URL} target="_blank" rel="noreferrer" className={LINK}>
						{translate("Authorized GitHub Apps")}
					</a>
					.
				</>
			}
		>
			{credential ? (
				<Forget
					device={signIn.device}
					credential={credential.id}
					what={translate("of GitHub on {{name}}", { name: signIn.name })}
					live={!offline}
					words={SIGN_OUT}
				/>
			) : null}
		</Row>
	);
};

/** Opens GitHub's page for a code once, when the code first shows. */
const useOpenOnce = (code: { userCode: string; verificationUri: string } | null) => {
	const opened = useRef<string | null>(null);
	const userCode = code?.userCode ?? null;
	const url = code?.verificationUri ?? null;
	useEffect(() => {
		if (userCode === null || url === null || opened.current === userCode) return;
		opened.current = userCode;
		window.open(url, "_blank", "noreferrer");
	}, [userCode, url]);
};

const SignInFlow = () => {
	useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const devices = signInDevices(view, now);
	const { state, begin, cancel } = useGithubSignIn();
	const [picked, setPicked] = useState<DeviceId | null>(null);
	const holder =
		picked !== null && devices.some((d) => d.device === picked)
			? picked
			: defaultHolder(view, devices);
	const waiting = state.state === "waiting" ? state : null;
	useOpenOnce(waiting);

	if (waiting) {
		return (
			<div className="flex flex-col gap-3 px-4 py-3.5">
				<span className="text-[12.5px] text-foreground">
					{translate("Enter this code on GitHub, then press Authorize.")}
				</span>
				<div className="flex items-center gap-3">
					<span className="font-mono text-[24px] font-[650] tracking-[0.12em] text-foreground-strong">
						{waiting.userCode}
					</span>
					<CopyButton text={waiting.userCode} className={SECONDARY} />
				</div>
				<div className="flex items-center gap-3">
					<span role="status" className="min-w-0 flex-1 text-xs text-muted-foreground">
						{translate("Waiting for GitHub. If its page didn't open,")}{" "}
						<a
							href={waiting.verificationUri}
							target="_blank"
							rel="noreferrer"
							className={LINK}
						>
							{translate("open it here")}
						</a>
						.
						<CopyIcon
							text={waiting.verificationUri}
							label={translate("Copy URL")}
							className="-my-1 ml-1 size-[18px]"
						/>
					</span>
					<button type="button" className={SECONDARY} onClick={() => void cancel()}>
						{translate("Cancel")}
					</button>
				</div>
			</div>
		);
	}

	if (state.state === "expired") {
		const slug = appSlugOn(view, state.device);
		return (
			<Row
				title={translate("The code ran out.")}
				sub={
					<>
						{translate(
							"GitHub refuses the code when the App's Enable Device Flow isn't ticked. Tick it in the App's settings under General, \"Identifying and authorizing users\"."
						)}
						{slug ? (
							<>
								{" "}
								<a
									href={appSettingsUrl(slug)}
									target="_blank"
									rel="noreferrer"
									className={LINK}
								>
									{translate("Open the App's settings")}
								</a>
							</>
						) : null}
					</>
				}
			>
				<button type="button" className={SECONDARY} onClick={() => void cancel()}>
					{translate("Cancel")}
				</button>
				<button type="button" className={PRIMARY} onClick={() => void begin(state.device)}>
					{translate("Try again")}
				</button>
			</Row>
		);
	}

	const starting = state.state === "starting";
	const failed = state.state === "failed" ? state : null;
	return (
		<Row
			title={translate("Sign in to GitHub")}
			sub={
				devices.length === 0
					? translate(
							"No connected device can hold the sign-in. Connect one, or update its maxd."
						)
					: translate("One device holds the sign-in. The App on it lets you sign in.")
			}
		>
			<StatusNote
				note={
					failed
						? {
								text: `${deviceName(view, failed.device)}: ${sentence(failed.reason)}.`,
								error: true,
							}
						: starting
							? {
									text: translate("Asking {{deviceName}} for a code", {
										deviceName: deviceName(view, state.device),
									}),
									error: false,
								}
							: null
				}
			/>
			{devices.length > 1 && holder ? (
				<Pick
					label={translate("Device")}
					value={holder}
					options={devices.map((d) => ({ value: d.device, label: d.name }))}
					onChange={setPicked}
				/>
			) : null}
			<button
				type="button"
				className={PRIMARY}
				disabled={!holder || starting}
				onClick={() => holder && void begin(holder)}
			>
				{devices.length === 1
					? translate("Sign in to GitHub on {{name}}", { name: devices[0]!.name })
					: translate("Sign in to GitHub")}
			</button>
		</Row>
	);
};

/** The "Merge and approve as you" card on the GitHub page. */
export const MergeAsYou = () => {
	useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const signIn = githubSignIn(view, now);
	return (
		<Section
			title={translate("Merge and approve as you")}
			hint={translate("Through your own GitHub sign-in")}
		>
			{/* The flow starts again from nothing after a sign-out. */}
			{signIn.state === "none" ? <SignInFlow /> : <SignedIn signIn={signIn} />}
		</Section>
	);
};
