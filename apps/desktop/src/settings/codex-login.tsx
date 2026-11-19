// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Codex in workspaces: each device that hosts workspaces signs in with
// ChatGPT on its own. The source runs the whole round, the browser and the
// callback included, so the page only starts it and says how it ended.

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import type { DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { codexStatus, sessionLine } from "@maxhq/shared/settings/login-parts";
import { sentence } from "@maxhq/shared/status/format";
import { isLive } from "@maxhq/shared/workspaces/state";

import { SignInAgain } from "./cli-sign-in";
import { Row, SECONDARY, Section, StatusNote } from "./controls";
import { type ForgetWords, HeldList } from "./login-parts";
import { Steps } from "./login-steps";
import { CODEX_USAGE_URL, heldOf, loginTargets } from "./logins";
import { failureOf, type Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const SIGN_OUT: ForgetWords = {
	get label() {
		return translate("Sign out");
	},
	get busy() {
		return translate("Signing out");
	},
};

const SignIn = () => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const targets = loginTargets(view, now);
	const held = heldOf(view, "chat_gpt");
	// One round at a time, since each opens the browser.
	const [running, setRunning] = useState<DeviceId | null>(null);
	const [notes, setNotes] = useState<ReadonlyMap<DeviceId, Note>>(() => new Map());
	const note = (device: DeviceId, value: Note | null) =>
		setNotes((n) => {
			const next = new Map(n);
			if (value) next.set(device, value);
			else next.delete(device);
			return next;
		});

	const signIn = async (device: DeviceId) => {
		setRunning(device);
		note(device, {
			text: translate("Waiting for you to sign in in the browser"),
			error: false,
		});
		try {
			const failure = failureOf(await source.signInChatGpt(device));
			note(
				device,
				failure
					? { text: `${sentence(failure)}.`, error: true }
					: { text: translate("Signed in"), error: false }
			);
		} catch (error) {
			console.warn(translate("Couldn't sign in to Codex"), error);
			note(device, {
				text: translate("Couldn't finish the sign-in. Try again."),
				error: true,
			});
		} finally {
			setRunning(null);
		}
	};

	return (
		<Section title={translate("Sign in")} hint={translate("Once on each device")}>
			{targets.length === 0 ? (
				<p className="px-4 py-3 text-xs text-muted-foreground">
					{translate("No device has Incus yet, so there's nowhere to sign in.")}
				</p>
			) : (
				targets.map((t) => {
					const session = held.find((c) => c.device === t.device);
					return (
						<Row key={t.device} title={t.name} sub={codexStatus(t, session)}>
							<StatusNote note={notes.get(t.device) ?? null} />
							<button
								type="button"
								className={SECONDARY}
								disabled={!t.live || running !== null}
								aria-label={translate("Continue with ChatGPT on {{name}}", {
									name: t.name,
								})}
								onClick={() => void signIn(t.device)}
							>
								{translate("Continue with ChatGPT")}
							</button>
						</Row>
					);
				})
			)}
		</Section>
	);
};

/** The page's headings and labels, for the settings search. */
export const CODEX_LOGIN_TERMS: readonly string[] = [
	"Sign in again",
	"Sign in",
	"Continue with ChatGPT",
	"Sessions devices hold",
	"Sign out",
];

/** The Codex in workspaces page. */
export const CodexLogin = () => {
	useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	return (
		<div className="flex flex-col gap-[18px]">
			<Steps login="codex" />
			<SignInAgain vendor="codex" />
			<SignIn />
			<Section
				title={translate("Sessions devices hold")}
				hint={translate("Sign out ends the session with OpenAI too")}
			>
				<HeldList
					view={view}
					held={heldOf(view, "chat_gpt")}
					now={now}
					live={(device) => isLive(view, device, now)}
					what={translate("the Codex session")}
					empty={translate("No device has signed in yet.")}
					describe={sessionLine}
					actions={() => (
						<a
							href={CODEX_USAGE_URL}
							target="_blank"
							rel="noreferrer"
							className="text-xs text-primary hover:underline"
						>
							{translate("Manage usage in ChatGPT settings")}
						</a>
					)}
					forget={SIGN_OUT}
				/>
			</Section>
		</div>
	);
};
