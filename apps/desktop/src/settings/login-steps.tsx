// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The one-time steps for each login, shown as written. Text between
// backticks shows as code: a web address as a link, and a command, a name
// or an address you type somewhere else with a copy icon. The page's text
// doesn't select, so the copy icon is how anything gets out.

import { Fragment } from "react";

import { InlineCopy } from "@/components/copy-field";

import type { LoginPage } from "./store";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** One login's steps, with what comes before and after them. */
export type LoginSteps = {
	readonly title: string;
	readonly before: readonly string[];
	readonly steps: readonly string[];
	readonly after: readonly string[];
};

/** The steps for each login. */
export const LOGIN_STEPS: Record<LoginPage, LoginSteps> = {
	claude: {
		get title() {
			return translate("Claude in workspaces");
		},
		get before() {
			return [
				translate(
					"Run the command in your own terminal. This mode is for personal use only, since it has MaxHQ keep a Claude credential for you."
				),
			];
		},
		get steps() {
			return [
				translate(
					"Open Terminal on the Mac and run the command the app shows. That's `claude setup-token` for the default login, or `CLAUDE_CONFIG_DIR=<dir> claude setup-token` for another account's folder."
				),
				translate(
					"Sign in in the browser with the same account as the MaxHQ account you picked."
				),
				translate(
					"Copy the token the terminal prints, paste it into MaxHQ's field, and tick the devices that host workspaces."
				),
				translate(
					"MaxHQ sends it to each ticked device's `maxd` once over the link, then clears the field. The app and the Mac keep no copy, and neither does the webview after the send."
				),
			];
		},
		get after() {
			return [
				translate(
					"Each device stores it as `secrets/claude-<origin>.token`, 0600. When an agent starts, MaxHQ hands it to the CLI as `CLAUDE_CODE_OAUTH_TOKEN`, so it lives only in the CLI's process. A project where you want Remote Control gets its own `claude auth login` in its container instead, and MaxHQ then gives it no token."
				),
			];
		},
	},
	codex: {
		get title() {
			return translate("Codex in workspaces");
		},
		before: [],
		get steps() {
			return [
				translate("In Settings, Logins, click Continue with ChatGPT next to a device."),
				translate(
					"MaxHQ asks that device's `maxd` for a sign-in. The device builds the authorize URL with `client_id=dynamic_agent_client`, `agent_name_hint=MaxHQ` and its own `ext_agent_host_id`, and keeps the PKCE verifier."
				),
				translate(
					"The app listens on 127.0.0.1 at the callback port and opens the browser. You sign in."
				),
				translate(
					"The app passes the code, the state and, on a first sign-in, the app ID OpenAI gave the device to that device, which exchanges them and stores the session in `secrets/chatgpt.json`, 0600."
				),
				translate("Repeat for each device. Each one keeps its own sign-in."),
			];
		},
		get after() {
			return [
				translate(
					"The refresh token never leaves the device that made it, since the terms ban sharing tokens. Containers never hold a token. Codex asks `maxd guest token codex` for each access token."
				),
				translate(
					"Sign out revokes the session with OpenAI, then deletes it from the device."
				),
			];
		},
	},
	cursor: {
		get title() {
			return translate("Cursor in workspaces");
		},
		get before() {
			return [
				translate(
					"On the Mac, Cursor agents run as the CLI's own login. Sign it in under Sign in again, or run `agent login` in Terminal once, and MaxHQ shows the account here."
				),
			];
		},
		get steps() {
			return [
				translate(
					"Open the Cursor dashboard at `https://cursor.com/dashboard`, go to Integrations, and create a user API key."
				),
				translate(
					"Paste it into MaxHQ's field, give it a name if you like, and tick the devices that host workspaces. The same key can go to every device."
				),
				translate(
					"MaxHQ sends it to each ticked device's `maxd` once over the link, then clears the field. The app and the Mac keep no copy."
				),
			];
		},
		get after() {
			return [
				translate(
					"Each device stores it as `secrets/cursor.json`, 0600. When a Cursor agent starts in a container, MaxHQ hands it to the CLI as `CURSOR_API_KEY`, so it lives only in the CLI's process. A device holds one key, so sending another replaces it."
				),
				translate(
					"Cursor shows no usage meters in MaxHQ. Manage usage opens the Cursor dashboard."
				),
			];
		},
	},
	github: {
		title: "GitHub App",
		before: [],
		get steps() {
			return [
				translate(
					"Open `https://github.com/settings/apps/new`, signed in to the GitHub account that owns your repos. For an organization's repos, open the organization's Settings, Developer settings, GitHub Apps instead."
				),
				translate(
					"GitHub App name: `MaxHQ-<your GitHub login>`. GitHub needs a name no other app uses."
				),
				translate("Homepage URL: `https://github.com/<your GitHub login>`."),
				translate(
					'Leave Callback URL empty. Leave "Request user authorization (OAuth) during installation" and "Enable Device Flow" unticked.'
				),
				translate("Under Webhook, untick Active."),
				translate(
					'Under Repository permissions, set Contents, Issues and Pull requests to "Read and write". Metadata becomes "Read-only" on its own. Leave every other repository, organization and account permission at "No access".'
				),
				translate(
					'Under "Where can this GitHub App be installed?", choose "Only on this account".'
				),
				translate(
					"Click Create GitHub App. Copy the App ID from the top of the app's page into MaxHQ."
				),
				translate(
					"Under Private keys, click Generate a private key. The browser saves a file ending in `.private-key.pem`."
				),
				translate(
					"In MaxHQ, choose that file and tick the devices that need it: each one that hosts workspaces, and any other whose agents look up pull requests and issues with `#`. MaxHQ reads it in its Rust side, never in the webview, sends it to each ticked device's `maxd`, and offers to move the downloaded file to the Trash. The Mac keeps no copy."
				),
				translate(
					"On the app's GitHub page, click Install App, choose your account, choose \"Only select repositories\", and pick the repos your MaxHQ projects use. Add more later on the same page. MaxHQ finds the installation itself, so you don't copy its ID."
				),
				translate(
					"MaxHQ checks the setup. Each device mints a token for one picked repo and lists the repos the App reaches."
				),
			];
		},
		get after() {
			return [
				translate(
					"Each device stores the key as `secrets/github-app.pem`, 0600 in a 0700 folder, next to `github-app.json` with the App ID and the key's SHA-256. The device never sends the key back, logs show only its SHA-256, and workspaces never see it. Only one-hour tokens leave the device, each limited to one repo with `contents: write`, `issues: write`, `pull_requests: write` and `metadata: read`, and each sent only to a running session in a workspace whose project has that repo. It's handled like the Claude and Codex tokens: kept only on the device that uses it, never forwarded, never in a container. Forget deletes both files."
				),
				translate(
					"A device without the App still finds pull requests and issues for `#` through `gh`, once you install it there and run `gh auth login`. MaxHQ only runs `gh api` reads, and `gh` keeps its own sign-in."
				),
				translate(
					'An App made before MaxHQ asked for Issues still works without it, and the check says so. To add it, open the App\'s Permissions & events page on GitHub, set Issues to "Read and write" and save. Then accept the request under Settings, Applications, Installed GitHub Apps, Configure.'
				),
			];
		},
	},
	apns: {
		get title() {
			return translate("Phone pushes");
		},
		get before() {
			return [
				translate(
					"Each device that runs agents pushes to your phones on its own, through Apple. It signs each push with an APNs key from your Apple developer account."
				),
			];
		},
		get steps() {
			return [
				translate(
					"Open `https://developer.apple.com/account/resources/authkeys/list`, signed in to the Apple developer account the MaxHQ phone app is built with."
				),
				"Click the plus button, give the key a name such as `MaxHQ pushes`, tick Apple Push Notifications service (APNs), and click Continue, then Register.",
				translate(
					"Copy the Key ID from the key's page into MaxHQ. It's 10 letters and digits, such as `ABC123DEFG`."
				),
				translate(
					"Click Download. The browser saves a file named `AuthKey_<Key ID>.p8`. Apple lets you download it only once."
				),
				translate(
					"Copy your team ID into MaxHQ. It's at the top right of the developer account, or under Membership details."
				),
				translate(
					"In MaxHQ, choose the `.p8` file and tick the devices that push to phones. MaxHQ reads it in its Rust side, never in the webview, sends it to each ticked device's `maxd`, and offers to move the downloaded file to the Trash. The Mac keeps no copy."
				),
			];
		},
		get after() {
			return [
				translate(
					"Each device keeps the key in its secrets folder, 0600, and never sends it back or logs it. What a push says is sealed to the phone's own key, so Apple can't read it."
				),
				translate(
					"One key works for every device and every phone. A device holds one key, so sending another replaces it. Forget deletes it, and that device stops pushing."
				),
			];
		},
	},
};

// A whole web address, without a placeholder to fill in.
const PAGE = /^https:\/\/[^\s<>]+$/;

// A command you run, or a name or an address with a placeholder you type
// into a form.
const TYPED = /^(claude |agent login|gh auth |CLAUDE_CONFIG_DIR=|MaxHQ[- ]|https:\/\/)/;

/** How a code span shows: a link or code with a copy icon, or plain code. */
export const spanKind = (part: string): "link" | "copy" | "code" =>
	PAGE.test(part) ? "link" : TYPED.test(part) ? "copy" : "code";

/** A line of the steps, with its code spans drawn as code. */
export const StepText = ({ text }: { text: string }) => {
	useI18n();
	return (
		<>
			{text.split("`").map((part, i) => {
				if (i % 2 === 0) return <Fragment key={i}>{part}</Fragment>;
				switch (spanKind(part)) {
					case "link":
						return <InlineCopy key={i} text={part} link />;
					case "copy":
						return <InlineCopy key={i} text={part} />;
					default:
						return (
							<code
								key={i}
								className="rounded-[4px] bg-secondary px-1 py-px font-mono text-[11.5px] break-all text-foreground"
							>
								{part}
							</code>
						);
				}
			})}
		</>
	);
};

/** A login's steps as a numbered list, with the notes before and after. */
export const Steps = ({ login }: { login: LoginPage }) => {
	useI18n();
	const { title, before, steps, after } = LOGIN_STEPS[login];
	const notes = [...before, ...after];
	return (
		<section
			aria-label={translate("Steps for {{title}}", { title: title })}
			className="overflow-hidden rounded-[11px] border border-track bg-card"
		>
			<div className="flex items-baseline justify-between gap-3 border-b border-sidebar-border px-4 py-3">
				<h2 className="text-[13.5px] font-[650] text-foreground">{translate("Steps")}</h2>
				<span className="text-[11.5px] text-muted-foreground">
					{translate("Once for each login")}
				</span>
			</div>
			<ol className="flex flex-col gap-2.5 px-4 py-3">
				{steps.map((step, i) => (
					<li
						key={i}
						className="flex gap-2.5 text-[12.5px] leading-[1.55] text-foreground"
					>
						<span
							aria-hidden
							className="grid size-[18px] shrink-0 place-items-center rounded-full bg-secondary text-[10.5px] font-[650] text-secondary-foreground tabular-nums"
						>
							{i + 1}
						</span>
						<span className="min-w-0">
							<StepText text={step} />
						</span>
					</li>
				))}
			</ol>
			{notes.length > 0 ? (
				<details className="group border-t border-sidebar-border px-4 py-2.5">
					<summary className="cursor-pointer text-[11.5px] text-muted-foreground select-none hover:text-foreground">
						{translate("How MaxHQ keeps it")}
					</summary>
					<div className="flex flex-col gap-2 pt-2">
						{notes.map((note, i) => (
							<p
								key={i}
								className="text-[12px] leading-[1.55] text-secondary-foreground"
							>
								<StepText text={note} />
							</p>
						))}
					</div>
				</details>
			) : null}
		</section>
	);
};
