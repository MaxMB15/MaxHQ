// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Claude in workspaces on the phone, as on the Mac: Sign in again for the
// logins a config folder holds, the setup-token you mint on the Mac sent
// to the devices that host workspaces, and the tokens they hold. The token
// sits only in its field until Send reads and clears it, and a device that
// has setup-tokens off, or doesn't take the phone's requests, can't be ticked.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { AccountId, DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { tokenLine, useTicks } from "@maxhq/shared/settings/login-parts";
import {
	heldOf,
	loginTargets,
	macAccounts,
	setupTokenCommand,
} from "@maxhq/shared/settings/logins";
import { SETUP_TOKENS_OFF, takesSetupTokens } from "@maxhq/shared/settings/setup-tokens";
import { accountName } from "@maxhq/shared/status/usage";
import { isLive } from "@maxhq/shared/workspaces/state";

import { useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Card } from "../../ui/Card";
import { CardBody, CopyTextButton, Muted } from "../devices/section";
import { Picker } from "../tasks/Picker";
import {
	DeviceTicks,
	HeldList,
	SecretField,
	SendFooter,
	SignInAgainCard,
	StepsCard,
	TextRow,
	useGated,
	useSecretSend,
} from "./login-parts";

const Send = ({ device }: { device?: DeviceId }) => {
	const { t } = useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const logins = macAccounts(view, "claude");
	const { targets, someOff } = useGated(
		loginTargets(view, now).map((target) =>
			takesSetupTokens(view, target.device)
				? target
				: { ...target, blocked: t(SETUP_TOKENS_OFF) }
		)
	);
	const [origin, setOrigin] = useState<AccountId | null>(logins[0]?.id ?? null);
	const login = logins.find((a) => a.id === origin) ?? logins[0];
	const [label, setLabel] = useState<string | null>(null);
	const [ticked, setTicked] = useTicks(targets, device);
	const name = (label ?? login?.label ?? "").trim();
	const devices = [...ticked];
	const secret = useSecretSend({
		deliver: (to, token) =>
			login ? source.storeClaudeToken(to, login.id, name, token) : Promise.resolve([]),
		what: t("setup-token"),
		retry: t("Couldn't send it. Mint a new token and try again."),
	});
	const ready = !!login && name !== "" && devices.length > 0;
	const command = login ? setupTokenCommand(login.config_dir) : null;

	return (
		<Card title={t("Send a token")} note={t("To each device you tick")}>
			<CardBody first>
				{logins.length === 0 ? (
					<Muted>
						{t(
							"No Claude login on the Mac yet. Sign in to Claude Code on the Mac first."
						)}
					</Muted>
				) : (
					<View style={styles.group}>
						<Text style={styles.label}>{t("Mac login")}</Text>
						<Picker
							label={t("Account")}
							value={login?.id ?? ""}
							options={logins.map((a) => ({
								value: a.id,
								label: a.label,
								detail: [accountName(a), a.email].filter(Boolean).join(", "),
							}))}
							onChange={(id) => setOrigin(id as AccountId)}
						/>
					</View>
				)}
				{command ? (
					<View style={styles.group}>
						<Text style={styles.label}>{t("Run this in Terminal on the Mac")}</Text>
						<Text accessibilityLabel={t("Command")} selectable style={styles.command}>
							{command}
						</Text>
						<View style={styles.row}>
							<CopyTextButton label={t("Copy command")} text={command} />
						</View>
					</View>
				) : null}
				<TextRow
					label={t("Name")}
					value={label ?? login?.label ?? ""}
					disabled={secret.busy}
					onChange={setLabel}
				/>
				<SecretField
					field={secret.field}
					onChange={secret.onChange}
					busy={secret.busy}
					label={t("Setup-token")}
					placeholder={t("Paste the token the terminal printed")}
				/>
				<DeviceTicks
					targets={targets}
					ticked={ticked}
					onChange={setTicked}
					disabled={secret.busy}
				/>
				<SendFooter
					view={view}
					secret={secret}
					ready={ready}
					someOff={someOff}
					devices={devices}
					done={() => t("New agents there run as this login.")}
				/>
			</CardBody>
		</Card>
	);
};

/** The Claude in workspaces page. `device` is the one the token form ticks alone. */
export const ClaudeLogin = ({ device }: { device?: DeviceId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	return (
		<>
			<StepsCard login="claude" />
			<SignInAgainCard vendor="claude" device={device} />
			<Send device={device} />
			<Card title={t("Tokens devices hold")} note={t("Forget deletes it on the device")}>
				<HeldList
					view={view}
					held={heldOf(view, "claude_token")}
					now={now}
					live={(d) => isLive(view, d, now)}
					what={t("the token")}
					empty={t("No device holds a token yet.")}
					describe={(c) => tokenLine(view, c)}
				/>
			</Card>
		</>
	);
};

const styles = StyleSheet.create({
	group: { gap: 6 },
	row: { flexDirection: "row" },
	label: { color: color.secondaryForeground, fontSize: size.meta, fontWeight: "600" },
	command: {
		fontFamily: font.mono,
		fontSize: size.meta,
		color: color.foreground,
		backgroundColor: color.subpanel,
		borderColor: color.border,
		borderWidth: StyleSheet.hairlineWidth,
		borderRadius: radius.sm,
		paddingHorizontal: 10,
		paddingVertical: 7,
	},
});
