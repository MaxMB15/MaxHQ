// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Cursor in workspaces on the phone, as on the Mac: Sign in again for the
// logins a config folder holds, the Cursor CLI's own login on the Mac, an
// API key sent to the devices that host workspaces, and the keys they hold.
// The key sits only in its field until Send reads and clears it.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";

import type { DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { useTicks } from "@maxhq/shared/settings/login-parts";
import { heldOf, loginTargets, macAccounts, signedInAs } from "@maxhq/shared/settings/logins";
import { accountName, CURSOR_USAGE_URL } from "@maxhq/shared/status/usage";
import { isLive } from "@maxhq/shared/workspaces/state";

import { useServices, useView } from "../../services";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { CardBody } from "../devices/section";
import {
	DeviceTicks,
	HeldList,
	openPage,
	SecretField,
	SendFooter,
	SignInAgainCard,
	StepNote,
	StepsCard,
	TextRow,
	useGated,
	useSecretSend,
} from "./login-parts";

const OnTheMac = () => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const logins = macAccounts(view, "cursor");
	return (
		<Card title={t("On the Mac")} note={t("The Cursor CLI's own login")}>
			{logins.length === 0 ? (
				<CardBody first>
					<StepNote
						text={t("Not signed in. Run `agent login` in Terminal on the Mac.")}
					/>
				</CardBody>
			) : (
				logins.map((a) => (
					<CardRow key={a.id} label={accountName(a)} note={signedInAs(a)}>
						<Button
							label={t("Manage usage")}
							tone="link"
							small
							onPress={() => openPage(CURSOR_USAGE_URL)}
						/>
					</CardRow>
				))
			)}
		</Card>
	);
};

const Send = ({ device }: { device?: DeviceId }) => {
	const { t } = useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const { targets, someOff } = useGated(loginTargets(view, now));
	const [label, setLabel] = useState("");
	const [ticked, setTicked] = useTicks(targets, device);
	const devices = [...ticked];
	const secret = useSecretSend({
		deliver: (to, key) => source.storeCursorKey(to, label.trim(), key),
		what: t("Cursor API key"),
		retry: t("Couldn't send it. Try again."),
	});

	return (
		<Card title={t("Send an API key")} note={t("To each device you tick")}>
			<CardBody first>
				<TextRow
					label={t("Name")}
					value={label}
					placeholder={t("API key")}
					disabled={secret.busy}
					onChange={setLabel}
				/>
				<SecretField
					field={secret.field}
					onChange={secret.onChange}
					busy={secret.busy}
					label={t("API key")}
					placeholder={t("Paste the key from your Cursor dashboard")}
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
					ready={devices.length > 0}
					someOff={someOff}
					devices={devices}
					done={() => t("New Cursor agents there use this key.")}
				/>
			</CardBody>
		</Card>
	);
};

/** The Cursor in workspaces page. `device` is the one the key form ticks alone. */
export const CursorLogin = ({ device }: { device?: DeviceId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	return (
		<>
			<StepsCard login="cursor" />
			<SignInAgainCard vendor="cursor" device={device} />
			<OnTheMac />
			<Send device={device} />
			<Card title={t("Keys devices hold")} note={t("Forget deletes it on the device")}>
				<HeldList
					view={view}
					held={heldOf(view, "cursor_key")}
					now={now}
					live={(d) => isLive(view, d, now)}
					what={t("the key")}
					empty={t("No device holds a key yet.")}
					describe={(c) => c.kind.label}
					actions={() => (
						<Button
							label={t("Manage usage")}
							tone="link"
							small
							onPress={() => openPage(CURSOR_USAGE_URL)}
						/>
					)}
				/>
			</Card>
		</>
	);
};
