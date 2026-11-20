// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Cursor in workspaces: the Mac runs agents as the CLI's own login, which
// signs in again from here, and each device that hosts workspaces gets a
// Cursor API key you paste. The key sits only in the field until Send
// reads it, clears it and hands it to the source.

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import type { DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { accountName, CURSOR_USAGE_URL } from "@maxhq/shared/status/usage";
import { isLive } from "@maxhq/shared/workspaces/state";

import { SignInAgain } from "./cli-sign-in";
import { FIELD, Label, Row, Section } from "./controls";
import {
	DeviceTicks,
	HeldList,
	SecretField,
	SendFooter,
	useSecretSend,
	useTicks,
} from "./login-parts";
import { Steps, StepText } from "./login-steps";
import { heldOf, loginTargets, macAccounts, signedInAs } from "./logins";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const ManageUsage = () => {
	useI18n();
	return (
		<a
			href={CURSOR_USAGE_URL}
			target="_blank"
			rel="noreferrer"
			className="text-xs text-primary hover:underline"
		>
			{translate("Manage usage")}
		</a>
	);
};

const OnTheMac = () => {
	useI18n();
	const view = useView((s) => s.view);
	const logins = macAccounts(view, "cursor");
	return (
		<Section title={translate("On the Mac")} hint={translate("The Cursor CLI's own login")}>
			{logins.length === 0 ? (
				<p className="px-4 py-3 text-xs text-muted-foreground">
					<StepText
						text={translate("Not signed in. Run `agent login` in Terminal on the Mac.")}
					/>
				</p>
			) : (
				logins.map((a) => (
					<Row key={a.id} title={accountName(a)} sub={signedInAs(a)}>
						<ManageUsage />
					</Row>
				))
			)}
		</Section>
	);
};

const Send = ({ device }: { device?: DeviceId }) => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const targets = loginTargets(view, now);
	const [label, setLabel] = useState("");
	const [ticked, setTicked] = useTicks(targets, device);
	const devices = [...ticked];
	const secret = useSecretSend({
		deliver: (to, key) => source.storeCursorKey(to, label.trim(), key),
		what: translate("Cursor API key"),
		retry: translate("Couldn't send it. Try again."),
	});
	const ready = devices.length > 0;

	return (
		<Section title={translate("Send an API key")} hint={translate("To each device you tick")}>
			<form
				className="flex flex-col gap-4 px-4 py-3.5"
				onSubmit={(e) => {
					e.preventDefault();
					if (ready) void secret.send(devices);
				}}
			>
				<label className="flex flex-col gap-1.5">
					<Label>{translate("Name")}</Label>
					<input
						value={label}
						onChange={(e) => setLabel(e.target.value)}
						placeholder={translate("API key")}
						spellCheck={false}
						autoComplete="off"
						className={FIELD}
					/>
				</label>

				<SecretField
					field={secret.field}
					onTyped={secret.setTyped}
					label={translate("API key")}
					placeholder={translate("Paste the key from your Cursor dashboard")}
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
					done={() => translate("New Cursor agents there use this key.")}
				/>
			</form>
		</Section>
	);
};

/** The page's headings and labels, for the settings search. */
export const CURSOR_LOGIN_TERMS: readonly string[] = [
	"Sign in again",
	"On the Mac",
	"Send an API key",
	"API key",
	"Keys devices hold",
];

/**
 * The Cursor in workspaces page. `device` is the one the key form ticks
 * alone, when Sign in again on its error opened the page.
 */
export const CursorLogin = ({ device }: { device?: DeviceId }) => {
	useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	return (
		<div className="flex flex-col gap-[18px]">
			<Steps login="cursor" />
			<SignInAgain vendor="cursor" />
			<OnTheMac />
			<Send device={device} />
			<Section
				title={translate("Keys devices hold")}
				hint={translate("Forget deletes it on the device")}
			>
				<HeldList
					view={view}
					held={heldOf(view, "cursor_key")}
					now={now}
					live={(device) => isLive(view, device, now)}
					what={translate("the key")}
					empty={translate("No device holds a key yet.")}
					describe={(c) => c.kind.label}
					actions={() => <ManageUsage />}
				/>
			</Section>
		</div>
	);
};
