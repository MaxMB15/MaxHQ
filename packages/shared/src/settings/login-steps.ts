// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The one-time steps for each login, shown as written on the Mac and on the
// phone, and the line under each login's name. Text between backticks
// shows as code: a web address as a link, and a command, a name or an
// address you type somewhere else with a way to copy it.

import { t } from "../i18n";

/** A login's page in Settings, Accounts. */
export type LoginPage = "claude" | "codex" | "cursor" | "github" | "apns";

/** The logins, in the order Settings lists them. */
export const LOGIN_PAGES: readonly LoginPage[] = ["claude", "codex", "cursor", "github", "apns"];

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
			return t("Claude in workspaces");
		},
		get before() {
			return [
				t(
					"Run the command in your own terminal. This mode is for personal use only, since it has MaxHQ keep a Claude credential for you."
				),
			];
		},
		get steps() {
			return [
				t(
					"Open Terminal on the Mac and run the command the app shows. That's `claude setup-token` for the default login, or `CLAUDE_CONFIG_DIR=<dir> claude setup-token` for another account's folder."
				),
				t("Sign in in the browser with the same account as the MaxHQ account you picked."),
				t(
					"Copy the token the terminal prints, paste it into MaxHQ's field, and tick the devices that host workspaces."
				),
				t(
					"MaxHQ sends it to each ticked device's `maxd` once over the link, then clears the field. The app and the Mac keep no copy, and neither does the webview after the send."
				),
			];
		},
		get after() {
			return [
				t(
					"Each device stores it as `secrets/claude-<origin>.token`, 0600. When an agent starts, MaxHQ hands it to the CLI as `CLAUDE_CODE_OAUTH_TOKEN`, so it lives only in the CLI's process. A project where you want Remote Control gets its own `claude auth login` in its container instead, and MaxHQ then gives it no token."
				),
			];
		},
	},
	codex: {
		get title() {
			return t("Codex in workspaces");
		},
		before: [],
		get steps() {
			return [
				t(
					"In Settings, Accounts, open Codex and click Continue with ChatGPT next to a device."
				),
				t(
					"MaxHQ asks that device's `maxd` for a sign-in. The device builds the authorize URL with `client_id=dynamic_agent_client`, `agent_name_hint=MaxHQ` and its own `ext_agent_host_id`, and keeps the PKCE verifier."
				),
				t(
					"The app listens on 127.0.0.1 at the callback port and opens the browser. You sign in."
				),
				t(
					"The app passes the code, the state and, on a first sign-in, the app ID OpenAI gave the device to that device, which exchanges them and stores the session in `secrets/chatgpt.json`, 0600."
				),
				t("Repeat for each device. Each one keeps its own sign-in."),
			];
		},
		get after() {
			return [
				t(
					"The refresh token never leaves the device that made it, since the terms ban sharing tokens. Containers never hold a token. Codex asks `maxd guest token codex` for each access token."
				),
				t("Sign out revokes the session with OpenAI, then deletes it from the device."),
			];
		},
	},
	cursor: {
		get title() {
			return t("Cursor in workspaces");
		},
		get before() {
			return [
				t(
					"On the Mac, Cursor agents run as the CLI's own login. Sign it in under Sign in again, or run `agent login` in Terminal once, and MaxHQ shows the account here."
				),
			];
		},
		get steps() {
			return [
				t(
					"Open the Cursor dashboard at `https://cursor.com/dashboard`, go to Integrations, and create a user API key."
				),
				t(
					"Paste it into MaxHQ's field, give it a name if you like, and tick the devices that host workspaces. The same key can go to every device."
				),
				t(
					"MaxHQ sends it to each ticked device's `maxd` once over the link, then clears the field. The app and the Mac keep no copy."
				),
			];
		},
		get after() {
			return [
				t(
					"Each device stores it as `secrets/cursor.json`, 0600. When a Cursor agent starts in a container, MaxHQ hands it to the CLI as `CURSOR_API_KEY`, so it lives only in the CLI's process. A device holds one key, so sending another replaces it."
				),
				t(
					"Cursor shows no usage meters in MaxHQ. Manage usage opens the Cursor dashboard."
				),
			];
		},
	},
	github: {
		get title() {
			return t("GitHub App");
		},
		before: [],
		get steps() {
			return [
				t(
					"Open `https://github.com/settings/apps/new`, signed in to the GitHub account that owns your repos. For an organization's repos, open the organization's Settings, Developer settings, GitHub Apps instead."
				),
				t(
					"GitHub App name: `MaxHQ-<your GitHub login>`. GitHub needs a name no other app uses."
				),
				t("Homepage URL: `https://github.com/<your GitHub login>`."),
				t(
					'Leave Callback URL empty. Leave "Request user authorization (OAuth) during installation" and "Enable Device Flow" unticked.'
				),
				t("Under Webhook, untick Active."),
				t(
					'Under Repository permissions, set Contents, Issues and Pull requests to "Read and write". Metadata becomes "Read-only" on its own. Leave every other repository, organization and account permission at "No access".'
				),
				t(
					'Under "Where can this GitHub App be installed?", choose "Only on this account".'
				),
				t(
					"Click Create GitHub App. Copy the App ID from the top of the app's page into MaxHQ."
				),
				t(
					"Under Private keys, click Generate a private key. The browser saves a file ending in `.private-key.pem`."
				),
				t(
					"In MaxHQ, choose that file and tick the devices that need it: each one that hosts workspaces, and any other whose agents look up pull requests and issues with `#`. MaxHQ reads it in its Rust side, never in the webview, sends it to each ticked device's `maxd`, and offers to move the downloaded file to the Trash. The Mac keeps no copy."
				),
				t(
					"On the app's GitHub page, click Install App, choose your account, choose \"Only select repositories\", and pick the repos your MaxHQ projects use. Add more later on the same page. MaxHQ finds the installation itself, so you don't copy its ID."
				),
				t(
					"MaxHQ checks the setup. Each device mints a token for one picked repo and lists the repos the App reaches."
				),
			];
		},
		get after() {
			return [
				t(
					"Each device stores the key as `secrets/github-app.pem`, 0600 in a 0700 folder, next to `github-app.json` with the App ID and the key's SHA-256. The device never sends the key back, logs show only its SHA-256, and workspaces never see it. Only one-hour tokens leave the device, each limited to one repo with `contents: write`, `issues: write`, `pull_requests: write` and `metadata: read`, and each sent only to a running session in a workspace whose project has that repo. It's handled like the Claude and Codex tokens: kept only on the device that uses it, never forwarded, never in a container. Forget deletes both files."
				),
				t(
					"A device without the App still finds pull requests and issues for `#` through `gh`, once you install it there and run `gh auth login`. MaxHQ only runs `gh api` reads, and `gh` keeps its own sign-in."
				),
				t(
					'An App made before MaxHQ asked for Issues still works without it, and the check says so. To add it, open the App\'s Permissions & events page on GitHub, set Issues to "Read and write" and save. Then accept the request under Settings, Applications, Installed GitHub Apps, Configure.'
				),
			];
		},
	},
	apns: {
		get title() {
			return t("Phone pushes");
		},
		get before() {
			return [
				t(
					"Each device that runs agents pushes to your phones on its own, through Apple. It signs each push with an APNs key from your Apple developer account."
				),
			];
		},
		get steps() {
			return [
				t(
					"Open `https://developer.apple.com/account/resources/authkeys/list`, signed in to the Apple developer account the MaxHQ phone app is built with."
				),
				t(
					"Click the plus button, give the key a name such as `MaxHQ pushes`, tick Apple Push Notifications service (APNs), and click Continue, then Register."
				),
				t(
					"Copy the Key ID from the key's page into MaxHQ. It's 10 letters and digits, such as `ABC123DEFG`."
				),
				t(
					"Click Download. The browser saves a file named `AuthKey_<Key ID>.p8`. Apple lets you download it only once."
				),
				t(
					"Copy your team ID into MaxHQ. It's at the top right of the developer account, or under Membership details."
				),
				t(
					"In MaxHQ, choose the `.p8` file and tick the devices that push to phones. MaxHQ reads it in its Rust side, never in the webview, sends it to each ticked device's `maxd`, and offers to move the downloaded file to the Trash. The Mac keeps no copy."
				),
			];
		},
		get after() {
			return [
				t(
					"Each device keeps the key in its secrets folder, 0600, and never sends it back or logs it. What a push says is sealed to the phone's own key, so Apple can't read it."
				),
				t(
					"One key works for every device and every phone. A device holds one key, so sending another replaces it. Forget deletes it, and that device stops pushing."
				),
			];
		},
	},
};

/** The line under each login's name in Settings and over its page. */
export const LOGIN_HINT: Record<LoginPage, string> = {
	get claude() {
		return t("A setup-token from a Mac login, for Claude Code in containers");
	},
	get codex() {
		return t("Each device's own Codex sign-in, for Codex in containers");
	},
	get cursor() {
		return t("The Mac's own Cursor login, and an API key for Cursor in containers");
	},
	get github() {
		return t("The App for pushes from workspaces and # lookups, and the repos it reaches");
	},
	get apns() {
		return t("The Apple key each device signs its pushes to your phones with");
	},
};

/**
 * The Firebase key's steps, for pushes to Android phones. It sits on the
 * Phones tab beside the APNs key and isn't a login page of its own, so
 * the logins each app lists stay as they are.
 */
export const FCM_STEPS: LoginSteps & { readonly hint: string } = {
	get title() {
		return t("Android pushes");
	},
	get hint() {
		return t("The Firebase key each device signs its pushes to Android phones with");
	},
	get before() {
		return [
			t(
				"Each device that runs agents pushes to your Android phones on its own, through Firebase Cloud Messaging. It signs in to Google with a service account key from the Firebase project the MaxHQ Android app is built with."
			),
		];
	},
	get steps() {
		return [
			t(
				"Open `https://console.firebase.google.com`, signed in to the Google account that owns the Firebase project the MaxHQ Android app is built with, and open that project."
			),
			t(
				"Click the gear beside Project Overview, choose Project settings, and open the Service accounts tab."
			),
			t(
				"Click Generate new private key, then Generate key. The browser saves a JSON file named like `<project>-firebase-adminsdk-<id>.json`. Firebase shows the key only this once."
			),
			t(
				"In MaxHQ, choose the JSON file and tick the devices that push to phones. MaxHQ reads it in its Rust side, never in the webview, sends the project ID, the service account's email and its private key to each ticked device's `maxd`, and offers to move the downloaded file to the Trash. The Mac keeps no copy."
			),
		];
	},
	get after() {
		return [
			t(
				"Each device keeps the key in its secrets folder, 0600, and never sends it back or logs it. What a push says is sealed to the phone's own key, so Google can't read it."
			),
			t(
				"One key works for every device and every Android phone. A device holds one Firebase key, so sending another replaces it. Forget deletes it, and that device stops pushing to Android phones. To revoke the key itself, delete it under the service account's Keys in the Google Cloud console."
			),
		];
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

/** A piece of a step: plain text, or a code span and how it shows. */
export type StepPart =
	| { readonly kind: "text"; readonly text: string }
	| { readonly kind: "link" | "copy" | "code"; readonly text: string };

/** A step's text split at its backticks, each code span with how it shows. */
export const stepParts = (text: string): StepPart[] =>
	text
		.split("`")
		.map((part, i): StepPart =>
			i % 2 === 0 ? { kind: "text", text: part } : { kind: spanKind(part), text: part }
		)
		.filter((part) => part.text !== "");
