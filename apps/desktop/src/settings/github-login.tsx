// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The GitHub App: its App ID and the key GitHub saved. Workspaces push with
// it, and `#` in the composer reads pull requests and issues with it, so it
// can go to any device, not only the ones that host workspaces. The key
// comes from the picker or a drop, since the backend reads only a
// path offered that way, and the page only ever holds that path. The source
// reads the file on the Rust side and sends it to each ticked device, which
// then lists the repos the App reaches. Once every device has it, the page
// offers to move the file to the Trash. Check again has a device look at
// its App now, rather than at its next check. Under the Apps, one device
// signs in to GitHub as you, to merge and approve PRs.

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import type { DeviceId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { listInWords, sentence } from "@maxhq/shared/status/format";
import type { ViewState } from "@maxhq/shared/view/replica";
import { isLive } from "@maxhq/shared/workspaces/state";

import { FIELD, Label, PRIMARY, SECONDARY, Section, StatusNote } from "./controls";
import {
	DeviceTicks,
	HeldList,
	KeyFileField,
	type Outcome,
	Outcomes,
	TrashOffer,
	useTicks,
	useTrash,
} from "./login-parts";
import { Steps } from "./login-steps";
import { MergeAsYou } from "./github-sign-in";
import { appTargets, heldOf, parseAppId } from "./logins";
import { failureOf, type Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** What a device's App reaches, in words. */
export const reachText = (repos: readonly string[]): string =>
	repos.length > 0
		? translate("Reaches {{listInWords}}.", { listInWords: listInWords(repos) })
		: translate("Reaches no repo yet. Install the App on the repos your projects use.");

// The App the device stored last, which is the one it just took.
const reachOn = (view: ViewState, device: DeviceId) => {
	const app = heldOf(view, "github_app")
		.filter((c) => c.device === device)
		.pop();
	return app ? reachText(app.kind.repos) : translate("The device hasn't listed the repos yet.");
};

// Asks the device to check its App now, such as right after you install
// it on a repo. The device answers once the check has started, and what it
// finds comes in with the App's next update.
const CheckAgain = ({ device, live }: { device: DeviceId; live: boolean }) => {
	useI18n();
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
			console.warn(translate("Couldn't ask for a check"), error);
			setNote({ text: translate("Couldn't reach the device. Try again."), error: true });
		} finally {
			setChecking(false);
		}
	};
	return (
		<>
			<StatusNote note={note} />
			<button
				type="button"
				className={SECONDARY}
				disabled={checking || !live}
				onClick={() => void check()}
			>
				{checking ? translate("Checking") : translate("Check again")}
			</button>
		</>
	);
};

const Send = () => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const targets = appTargets(view, now);
	const [appId, setAppId] = useState("");
	const [keyPath, setKeyPath] = useState<string | null>(null);
	const [ticked, setTicked] = useTicks(targets);
	const [busy, setBusy] = useState(false);
	const [outcomes, setOutcomes] = useState<Outcome[]>([]);
	const [trash, setTrash, move] = useTrash(() => setKeyPath(null));
	const [note, setNote] = useState<Note | null>(null);

	const id = parseAppId(appId);
	const devices = [...ticked];
	const ready = id !== null && keyPath !== null && devices.length > 0 && !busy;

	const send = async () => {
		if (!ready || id === null || keyPath === null) return;
		setBusy(true);
		setNote(null);
		setOutcomes([]);
		setTrash(null);
		try {
			const results = await source.setGithubApp(devices, id, keyPath);
			const done = devices.map((device, i) => {
				const result = results[i];
				return {
					device,
					failure: result ? failureOf(result) : translate("The device didn't answer"),
				};
			});
			setOutcomes(done);
			if (done.every((o) => o.failure === null)) setTrash({ step: "offer", path: keyPath });
		} catch (error) {
			console.warn(translate("Couldn't send the GitHub App"), error);
			setNote({
				text: translate("Couldn't send it. Check the file and try again."),
				error: true,
			});
		} finally {
			setBusy(false);
		}
	};

	return (
		<Section title={translate("Send the App")} hint={translate("To each device you tick")}>
			<form
				className="flex flex-col gap-4 px-4 py-3.5"
				onSubmit={(e) => {
					e.preventDefault();
					void send();
				}}
			>
				<label className="flex flex-col gap-1.5">
					<Label>{translate("App ID")}</Label>
					<input
						aria-label={translate("App ID")}
						value={appId}
						onChange={(e) => setAppId(e.target.value)}
						inputMode="numeric"
						placeholder={translate("From the top of the App's page")}
						spellCheck={false}
						autoComplete="off"
						className={cn(FIELD, "w-[220px] font-mono text-xs")}
					/>
					{appId.trim() !== "" && id === null ? (
						<span className="text-[11.5px] text-status-error-text">
							{translate("The App ID is a number, such as 1234567.")}
						</span>
					) : null}
				</label>

				<KeyFileField
					label={translate("Private key")}
					path={keyPath}
					onPath={setKeyPath}
					onFailure={setNote}
					empty={translate("Drop the .private-key.pem file here, or choose it")}
				/>

				<DeviceTicks
					targets={targets}
					ticked={ticked}
					onChange={setTicked}
					disabled={busy}
					label={translate("Devices")}
					empty={translate(
						"MaxHQ doesn't know any device yet, so there's nowhere to send it."
					)}
				/>

				<div className="flex items-start gap-3">
					<div className="flex min-w-0 flex-1 flex-col gap-2">
						<StatusNote note={note} className="text-xs" />
						<Outcomes
							view={view}
							outcomes={outcomes}
							done={(device) => reachOn(view, device)}
						/>
						{trash ? (
							<TrashOffer trash={trash} onMove={() => void move(trash.path)} />
						) : null}
					</div>
					<button type="submit" className={PRIMARY} disabled={!ready}>
						{busy ? translate("Sending") : translate("Send")}
					</button>
				</div>
			</form>
		</Section>
	);
};

/** The page's headings and labels, for the settings search. */
export const GITHUB_LOGIN_TERMS: readonly string[] = [
	"Send the App",
	"App ID",
	"Private key",
	"Apps devices hold",
	"Merge and approve as you",
	"Sign in to GitHub",
];

/** The GitHub App page. */
export const GithubLogin = () => {
	useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	return (
		<div className="flex flex-col gap-[18px]">
			<Steps login="github" />
			<Send />
			<Section
				title={translate("Apps devices hold")}
				hint={translate("Forget deletes the key on the device")}
			>
				<HeldList
					view={view}
					held={heldOf(view, "github_app")}
					now={now}
					live={(device) => isLive(view, device, now)}
					what={translate("the GitHub App")}
					empty={translate("No device holds a GitHub App yet.")}
					actions={(c) => (
						<CheckAgain device={c.device} live={isLive(view, c.device, now)} />
					)}
					describe={(c) => (
						<>
							<span className="block">
								{translate("App {{app_id}}{{value2}}. Key SHA-256 {{value3}}…", {
									app_id: String(c.kind.app_id),
									value2: c.kind.slug ? `, ${c.kind.slug}` : "",
									value3: c.kind.key_sha256.slice(0, 12),
								})}
							</span>
							<span className="block">{reachText(c.kind.repos)}</span>
						</>
					)}
				/>
			</Section>
			<MergeAsYou />
		</div>
	);
};
