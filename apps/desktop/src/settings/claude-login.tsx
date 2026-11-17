// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Claude in workspaces: the setup-token you mint in your own terminal for a
// login on the Mac, sent to the devices that host workspaces. The token
// field is left to the browser, so the token sits only in the field until
// Send reads it, clears it and hands it to the source. No state, store or
// log ever gets it, and a reply that echoes it shows `<secret>` instead.
// A device takes one only once you turn setup-tokens on in its settings,
// so one that has them off can't be ticked.

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import type { AccountId, DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { Picker } from "@/new-agent/picker";
import { tokenLine } from "@maxhq/shared/settings/login-parts";
import { SETUP_TOKENS_OFF, takesSetupTokens } from "@maxhq/shared/settings/setup-tokens";
import { accountName } from "@maxhq/shared/status/usage";
import { isLive } from "@maxhq/shared/workspaces/state";

import { SignInAgain } from "./cli-sign-in";
import { FIELD, Label, SECONDARY, Section } from "./controls";
import {
	DeviceTicks,
	HeldList,
	SecretField,
	SendFooter,
	useSecretSend,
	useTicks,
} from "./login-parts";
import { Steps } from "./login-steps";
import { heldOf, loginTargets, macAccounts, setupTokenCommand } from "./logins";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const Send = ({ device }: { device?: DeviceId }) => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const logins = macAccounts(view, "claude");
	const targets = loginTargets(view, now).map((t) =>
		takesSetupTokens(view, t.device) ? t : { ...t, blocked: translate(SETUP_TOKENS_OFF) }
	);
	const [origin, setOrigin] = useState<AccountId | null>(logins[0]?.id ?? null);
	const login = logins.find((a) => a.id === origin);
	const [label, setLabel] = useState<string | null>(null);
	const [ticked, setTicked] = useTicks(targets, device);
	const name = (label ?? login?.label ?? "").trim();
	const devices = [...ticked];
	const secret = useSecretSend({
		deliver: (to, token) =>
			login ? source.storeClaudeToken(to, login.id, name, token) : Promise.resolve([]),
		what: translate("setup-token"),
		retry: translate("Couldn't send it. Mint a new token and try again."),
	});
	const ready = !!login && name !== "" && devices.length > 0;

	const command = login ? setupTokenCommand(login.config_dir) : null;

	return (
		<Section title={translate("Send a token")} hint={translate("To each device you tick")}>
			<form
				className="flex flex-col gap-4 px-4 py-3.5"
				onSubmit={(e) => {
					e.preventDefault();
					if (ready) void secret.send(devices);
				}}
			>
				{logins.length === 0 ? (
					<p className="text-xs text-muted-foreground">
						{translate(
							"No Claude login on the Mac yet. Sign in to Claude Code on the Mac first."
						)}
					</p>
				) : (
					<div className="flex flex-col gap-1.5">
						<Label>{translate("Mac login")}</Label>
						<div className="flex flex-wrap items-center gap-2">
							<Picker
								label={translate("Account")}
								value={origin ?? ""}
								options={logins.map((a) => ({
									value: a.id,
									label: a.label,
									detail: [accountName(a), a.email].filter(Boolean).join(", "),
								}))}
								onChange={(id) => setOrigin(id)}
							/>
						</div>
					</div>
				)}

				{command ? (
					<div className="flex flex-col gap-1.5">
						<Label>{translate("Run this in Terminal on the Mac")}</Label>
						<div className="flex items-center gap-2">
							<code
								aria-label={translate("Command")}
								className="min-w-0 flex-1 truncate rounded-[7px] border border-border bg-subpanel px-2.5 py-[7px] font-mono text-xs text-foreground"
							>
								{command}
							</code>
							<CopyButton
								text={command}
								label={translate("Copy command")}
								icon={false}
								className={SECONDARY}
							/>
						</div>
					</div>
				) : null}

				<label className="flex flex-col gap-1.5">
					<Label>{translate("Name")}</Label>
					<input
						value={label ?? login?.label ?? ""}
						onChange={(e) => setLabel(e.target.value)}
						spellCheck={false}
						autoComplete="off"
						className={FIELD}
					/>
				</label>

				<SecretField
					field={secret.field}
					onTyped={secret.setTyped}
					label={translate("Setup-token")}
					placeholder={translate("Paste the token the terminal printed")}
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
					done={() => translate("New agents there run as this login.")}
				/>
			</form>
		</Section>
	);
};

/** The page's headings and labels, for the settings search. */
export const CLAUDE_LOGIN_TERMS: readonly string[] = [
	"Sign in again",
	"Send a token",
	"Mac login",
	"Account",
	"Run this in Terminal on the Mac",
	"Setup-token",
	"Tokens devices hold",
];

/**
 * The Claude in workspaces page. `device` is the one the token form ticks
 * alone, when Sign in again on its error opened the page.
 */
export const ClaudeLogin = ({ device }: { device?: DeviceId }) => {
	useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	return (
		<div className="flex flex-col gap-[18px]">
			<Steps login="claude" />
			<SignInAgain vendor="claude" />
			<Send device={device} />
			<Section
				title={translate("Tokens devices hold")}
				hint={translate("Forget deletes it on the device")}
			>
				<HeldList
					view={view}
					held={heldOf(view, "claude_token")}
					now={now}
					live={(device) => isLive(view, device, now)}
					what={translate("the token")}
					empty={translate("No device holds a token yet.")}
					describe={(c) => tokenLine(view, c)}
				/>
			</Section>
		</div>
	);
};
