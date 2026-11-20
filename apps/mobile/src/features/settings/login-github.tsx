// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The GitHub page on the phone. The App goes to devices from the Mac, since
// only a file the Mac's picker offers can be sent and the phone has none,
// so the phone lists the Apps devices hold, with Check again and Forget.
// Under them, one device signs in to GitHub as you, to merge and approve:
// the phone starts it, shows the code to enter on GitHub, and waits until
// that device holds your login.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { DeviceId } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import { useNow } from "@maxhq/shared/lib/clock";
import { type GithubSignIn, githubSignIn, signInDevices } from "@maxhq/shared/pulls/devices";
import {
	appSettingsUrl,
	appSlugOn,
	AUTHORIZED_APPS_URL,
	defaultHolder,
	useGithubSignIn,
} from "@maxhq/shared/settings/github-sign-in";
import { appLines } from "@maxhq/shared/settings/login-parts";
import { deviceFirst, heldOf } from "@maxhq/shared/settings/logins";
import { sentence } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";
import { isLive } from "@maxhq/shared/workspaces/state";

import { useServices, useView } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { Picker } from "../tasks/Picker";
import { Buttons, CardBody, CopyTextButton, Muted, type Note, NoteLine } from "../devices/section";
import { Forget, HeldList, openPage, StepsCard, useSignOutWords } from "./login-parts";

// Asks the device to check its App now, such as right after you install it
// on a repo. What it finds comes in with the App's next update.
const CheckAgain = ({ device, live }: { device: DeviceId; live: boolean }) => {
	const { t } = useI18n();
	const { source } = useServices();
	const [checking, setChecking] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const check = async () => {
		setChecking(true);
		setNote(null);
		try {
			const failure = failureOf(await source.checkGithubApp(device));
			setNote(failure ? { text: `${sentence(failure)}.`, error: true } : null);
		} catch (error) {
			console.warn(t("Couldn't ask for a check"), error);
			setNote({ text: t("Couldn't reach the device. Try again."), error: true });
		} finally {
			setChecking(false);
		}
	};
	return (
		<>
			<Button
				label={checking ? t("Checking") : t("Check again")}
				small
				disabled={checking || !live}
				onPress={() => void check()}
			/>
			<NoteLine note={note} />
		</>
	);
};

const SignedIn = ({ signIn }: { signIn: Exclude<GithubSignIn, { state: "none" }> }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const lets = useLetsPhone(signIn.device);
	const signOut = useSignOutWords();
	const credential = heldOf(view, "github_user").find((c) => c.device === signIn.device);
	const offline = signIn.state === "offline";
	return (
		<CardBody first>
			<Text style={styles.title}>
				{t("Signed in as @{{login}} on {{name}}{{value3}}", {
					login: signIn.login,
					name: signIn.name,
					value3: offline ? t(", which is offline") : "",
				})}
			</Text>
			<Text style={styles.meta}>
				{t("Merges and approvals go through ")}
				{signIn.name}{" "}
				{t(" as you. Sign out deletes the sign-in there. To revoke it on GitHub too, use")}{" "}
				<Text
					accessibilityRole="link"
					style={styles.link}
					onPress={() => openPage(AUTHORIZED_APPS_URL)}
				>
					{t("Authorized GitHub Apps")}
				</Text>
				.
			</Text>
			{credential ? (
				<>
					<Buttons>
						<Forget
							device={signIn.device}
							credential={credential.id}
							what={t("of GitHub on {{name}}", { name: signIn.name })}
							live={!offline}
							lets={lets}
							words={signOut}
						/>
					</Buttons>
					{lets ? null : <NeedsSwitch />}
				</>
			) : null}
		</CardBody>
	);
};

const SignInFlow = ({ device }: { device?: DeviceId }) => {
	const { t } = useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const devices = deviceFirst(signInDevices(view, now), device);
	const { state, begin, cancel } = useGithubSignIn(source, view);
	const [picked, setPicked] = useState<DeviceId | null>(null);
	const holder =
		picked !== null && devices.some((d) => d.device === picked)
			? picked
			: device !== undefined && devices.some((d) => d.device === device)
				? device
				: defaultHolder(view, devices);
	const lets = useLetsPhone(holder ?? null);

	if (state.state === "waiting") {
		return (
			<CardBody first>
				<Text style={styles.body}>
					{t("Enter this code on GitHub, then press Authorize.")}
				</Text>
				<View style={styles.codeRow}>
					<Text selectable style={styles.code}>
						{state.userCode}
					</Text>
					<CopyTextButton label={t("Copy")} text={state.userCode} />
				</View>
				<Text accessibilityLiveRegion="polite" style={styles.meta}>
					{t("Waiting for you to sign in")}
				</Text>
				<Buttons>
					<Button
						label={t("Open the sign-in page")}
						tone="primary"
						small
						onPress={() => openPage(state.verificationUri)}
					/>
					<CopyTextButton label={t("Copy URL")} text={state.verificationUri} />
					<Button label={t("Cancel")} small onPress={() => void cancel()} />
				</Buttons>
			</CardBody>
		);
	}

	if (state.state === "expired") {
		const slug = appSlugOn(view, state.device);
		return (
			<CardBody first>
				<Text style={styles.title}>{t("The code ran out.")}</Text>
				<Text style={styles.meta}>
					{t(
						"GitHub refuses the code when the App's Enable Device Flow isn't ticked. Tick it in the App's settings under General, \"Identifying and authorizing users\"."
					)}
				</Text>
				<Buttons>
					{slug ? (
						<Button
							label={t("Open the App's settings")}
							tone="link"
							small
							onPress={() => openPage(appSettingsUrl(slug))}
						/>
					) : null}
					<Button label={t("Cancel")} small onPress={() => void cancel()} />
					<Button
						label={t("Try again")}
						tone="primary"
						small
						onPress={() => void begin(state.device)}
					/>
				</Buttons>
			</CardBody>
		);
	}

	const starting = state.state === "starting";
	const failed = state.state === "failed" ? state : null;
	return (
		<CardBody first>
			<Text style={styles.title}>{t("Sign in to GitHub")}</Text>
			<Text style={styles.meta}>
				{devices.length === 0
					? t(
							"No connected device can hold the sign-in. Connect one, or update its maxd."
						)
					: t("One device holds the sign-in. The App on it lets you sign in.")}
			</Text>
			<NoteLine
				note={
					failed
						? {
								text: `${deviceName(view, failed.device)}: ${sentence(failed.reason)}.`,
								error: true,
							}
						: starting
							? {
									text: t("Asking {{deviceName}} for a code", {
										deviceName: deviceName(view, state.device),
									}),
									error: false,
								}
							: null
				}
			/>
			{devices.length > 1 && holder ? (
				<Picker
					label={t("Device")}
					value={holder}
					options={devices.map((d) => ({ value: d.device, label: d.name }))}
					onChange={(value) => setPicked(value as DeviceId)}
				/>
			) : null}
			<Buttons>
				<Button
					label={
						devices.length === 1
							? t("Sign in to GitHub on {{name}}", { name: devices[0]!.name })
							: t("Sign in to GitHub")
					}
					tone="primary"
					disabled={!holder || starting || !lets}
					onPress={() => {
						if (holder) void begin(holder);
					}}
				/>
			</Buttons>
			{holder && !lets ? <NeedsSwitch /> : null}
		</CardBody>
	);
};

/** Merge and approve as you, through one device's GitHub sign-in. */
const MergeAsYou = ({ device }: { device?: DeviceId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const signIn = githubSignIn(view, now);
	return (
		<Card title={t("Merge and approve as you")} note={t("Through your own GitHub sign-in")}>
			{/* The flow starts again from nothing after a sign-out. */}
			{signIn.state === "none" ? (
				<SignInFlow device={device} />
			) : (
				<SignedIn signIn={signIn} />
			)}
		</Card>
	);
};

/** The GitHub page. `device` puts that device's App first. */
export const GithubLogin = ({ device }: { device?: DeviceId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	return (
		<>
			<StepsCard login="github" />
			<Card title={t("Send the App")} note={t("To each device you tick")}>
				<CardBody first>
					<Muted>
						{t(
							"Send the App from Settings on your Mac. The phone can't read the key file."
						)}
					</Muted>
				</CardBody>
			</Card>
			<Card title={t("Apps devices hold")} note={t("Forget deletes the key on the device")}>
				<HeldList
					view={view}
					held={deviceFirst(heldOf(view, "github_app"), device)}
					now={now}
					live={(d) => isLive(view, d, now)}
					what={t("the GitHub App")}
					empty={t("No device holds a GitHub App yet.")}
					actions={(c) => (
						<CheckAgain device={c.device} live={isLive(view, c.device, now)} />
					)}
					describe={appLines}
				/>
			</Card>
			<MergeAsYou device={device} />
		</>
	);
};

const styles = StyleSheet.create({
	title: { color: color.foreground, fontSize: size.body, fontWeight: "600" },
	body: { color: color.foreground, fontSize: size.small },
	meta: { color: color.mutedForeground, fontSize: size.meta, lineHeight: 18 },
	link: { color: color.primary },
	codeRow: { flexDirection: "row", alignItems: "center", gap: 12 },
	code: {
		fontFamily: font.mono,
		fontSize: 24,
		fontWeight: "600",
		letterSpacing: 2.5,
		color: color.foregroundStrong,
	},
});
