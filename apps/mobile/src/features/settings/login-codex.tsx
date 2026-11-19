// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Codex in workspaces on the phone. Each device that hosts workspaces signs
// in on its own, through a browser round the Mac runs with its callback, so
// the phone shows where each stands and says that sign-in is on the Mac.
// Sign in again runs a config folder's CLI login, and the sessions devices
// hold can be signed out from here.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";

import type { DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { codexStatus, sessionLine } from "@maxhq/shared/settings/login-parts";
import { CODEX_USAGE_URL, deviceFirst, heldOf, loginTargets } from "@maxhq/shared/settings/logins";
import { isLive } from "@maxhq/shared/workspaces/state";

import { useView } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { CardBody, Muted } from "../devices/section";
import { HeldList, openPage, SignInAgainCard, StepsCard, useSignOutWords } from "./login-parts";

const SignIn = ({ device }: { device?: DeviceId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const targets = deviceFirst(loginTargets(view, now), device);
	const held = heldOf(view, "chat_gpt");
	return (
		<Card title={t("Sign in")} note={t("Once on each device")}>
			{targets.length === 0 ? (
				<CardBody first>
					<Muted>{t("No device has Incus yet, so there's nowhere to sign in.")}</Muted>
				</CardBody>
			) : (
				targets.map((target, i) => (
					<CardBody key={target.device} first={i === 0}>
						<View style={styles.row}>
							<View style={styles.text}>
								<Text style={styles.name}>{target.name}</Text>
								<Text style={styles.meta}>
									{codexStatus(
										target,
										held.find((c) => c.device === target.device)
									)}
								</Text>
							</View>
							<Text
								accessibilityHint={t("Sign in from Settings on your Mac.")}
								style={styles.meta}
							>
								{t("On the Mac")}
							</Text>
						</View>
					</CardBody>
				))
			)}
		</Card>
	);
};

/** The Codex in workspaces page. `device` puts that device first. */
export const CodexLogin = ({ device }: { device?: DeviceId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const signOut = useSignOutWords();
	return (
		<>
			<StepsCard login="codex" />
			<SignInAgainCard vendor="codex" device={device} />
			<SignIn device={device} />
			<Card
				title={t("Sessions devices hold")}
				note={t("Sign out ends the session with OpenAI too")}
			>
				<HeldList
					view={view}
					held={deviceFirst(heldOf(view, "chat_gpt"), device)}
					now={now}
					live={(d) => isLive(view, d, now)}
					what={t("the Codex session")}
					empty={t("No device has signed in yet.")}
					describe={sessionLine}
					actions={() => (
						<Button
							label={t("Manage usage in ChatGPT settings")}
							tone="link"
							small
							onPress={() => openPage(CODEX_USAGE_URL)}
						/>
					)}
					forget={signOut}
				/>
			</Card>
		</>
	);
};

const styles = StyleSheet.create({
	row: { flexDirection: "row", alignItems: "center", gap: 10 },
	text: { flex: 1, minWidth: 0 },
	name: { color: color.foreground, fontSize: size.body },
	meta: { color: color.mutedForeground, fontSize: size.meta },
});
