<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Settings

What each Settings page holds, where to find it, and what its labels do. A device's own settings sit on its page, and a project's settings in its own page of Settings.

## Open the settings
<!-- id: settings/open -->
<!-- covers: view/settings -->

Click the gear at the top of the window, or press `⌘,` (`Ctrl+,` off a Mac). The list on the left picks the page.

- The list has three groups: App for the app's own pages, Logins for the sign-ins workspaces use, and Projects with one page per project.
- Type in Search, or press `⌘F` while Settings is open, to narrow the list to the pages whose titles, headings or labels have your words. Enter opens the first one.
- While you search, the devices whose own settings have your words show under Devices. Picking one closes Settings and opens that device's page at the card that matched.
- Esc clears the search first, and closes Settings once the search is empty.

## Where each setting is kept
<!-- id: settings/where-kept -->

A page whose line says "kept on this Mac" saves on this Mac only. The other pages change what your devices do, and each change goes to the devices it names.

- Kept on this Mac: Appearance, Sidebar, Editor and Dispatcher, plus Notifications and the Dock badge.
- On your devices: Agent tools, Scheduled tasks, Test hosts, Skills, Phones and every login page.
- A device's own settings, such as idle agents, memory, sleep and network access, are on that device's page. See [Devices and links](devices.md).
- A project's settings and its skills are on the project's page under Projects. See [Projects and workspaces](projects.md).

## Appearance: theme and language
<!-- id: settings/appearance -->
<!-- covers: settings/appearance -->

Appearance sets the app's colors and the language of its interface. Both are kept on this Mac and apply at once.

- Colors, Theme: System, Light or Dark. System follows the Mac's appearance as it changes through the day.
- Language, Interface language: System, English, Nederlands, Deutsch, Français or Español. System follows this device's language.

## Sidebar: Working shelf and snooze options
<!-- id: settings/sidebar -->
<!-- covers: settings/sidebar -->

The Sidebar page sets how the sidebar lists agents: whether busy agents fold away, and the choices in every snooze menu. It's kept on this Mac.

- Shelves, Working shelf: turn it on to fold busy agents into a Working shelf. They come back to the top when they need you.
- Snooze options: the choices in every snooze menu, listed soonest first. Each one is In an amount of minutes, hours or days, or Today, Tomorrow or a weekday at a time. A weekday means the next one after today.
- Add option adds one, up to 8. The cross next to an option removes it. Reset to defaults brings back the starting list.

## Editor: Open in, spelling, Enter and port prompts
<!-- id: settings/editor -->
<!-- covers: settings/editor -->

The Editor page picks the editor workspaces and files open in, how spelling is checked as you type, what Enter does while an agent works, and whether an agent's view offers its ports. It's kept on this Mac.

- Open in, Default editor: the editor the Open in button and the buttons on file rows use. Pick Last used, Cursor, VS Code or None. Last used is the one you opened last. None hides those buttons. An editor this Mac doesn't have says "not found".
- Typing, Check spelling: underlines misspelled words in the composer and in notes, with the Mac's guesses on a right-click.
- Correct spelling automatically: lets macOS fix words, curl quotes and expand text replacements as you type. It needs Check spelling.
- While an agent works, Enter: Queue waits until the agent finishes, then sends. Steer sends at once, and the agent reads it between steps. `⌘⏎` does the other one.
- Conversation, Port prompts: offers to forward or open a port an agent starts listening on, at the top of its view. Off, ports still show in each agent's header. See [Ports and forwards](ports.md).

## Dispatcher: its account and whether it acts directly
<!-- id: settings/dispatcher -->
<!-- covers: settings/dispatcher -->

The Dispatcher page picks which Claude or Codex account answers the box at the top of the window, and whether the dispatcher makes changes without asking. It's kept on this Mac.

- Account, Runs as: pick one of this Mac's Claude or Codex accounts. Without a pick, the dispatcher takes the first Claude account, else the first Codex one.
- With no account on this Mac, the page says to sign in to Claude or Codex first.
- Changes, Act directly: the dispatcher changes settings, agents and tasks without asking first. Deleting, stopping busy agents and granting permissions still ask. Off, every change waits in a plan you confirm. See [The dispatcher](dispatcher.md).
- The dispatcher reads your projects and agents, and can't change your files.

## Agent tools: limits, allowed agents and access keys
<!-- id: settings/agent-tools -->
<!-- covers: settings/agent-tools -->

Agent tools sets what agents and outside tools may do when they run agents through MaxHQ: the limits new permissions start with, the agents allowed to run agents, and access keys.

- New permissions: Depth, Agents started, Agents at once and Usage cap, which every new permission starts with. They start 3 levels deep with no limits. An empty limit is off. Save defaults saves them.
- Depth counts the levels of agents below the one that holds the permission. Limits count agents and usage. They don't fence off files: an agent that should stay away from your files runs in a container, with its permission's Containers setting on Ask or Never.
- Agents allowed to run agents: each agent you allowed, with Revoke. Allow one from its agent's menu, with Allow to run agents.
- Access keys, for tools outside MaxHQ, such as a bot: New access key takes a Key name and the device it's kept on, then the Devices, Vendors, Accounts, Places and Test hosts it may use, Containers, and limits that include Expires after a number of days. Make key makes it.
- Each key shows where its file is, with a copy button for the path, Copy MCP command for the command a tool runs, and Revoke. The page never shows the key itself.

## Scheduled tasks: prompts on a timer or a webhook
<!-- id: settings/scheduled-tasks -->
<!-- covers: settings/scheduled-tasks -->

Scheduled tasks lists every device's tasks: prompts a device sends to an agent on a timer, on a webhook or on GitHub events. New task makes one.

- New task opens the form: a Name, the Prompt, When it runs (Every some minutes, At a time on chosen days, Webhook or GitHub events), and Where it goes (Sends to and Lives on).
- A webhook task has a Webhook part: Take requests turns it on or off, and Requests it takes says which requests start a run. The URL shows once, after you save.
- Each task shows when it runs next and how it last went, with History, Run now, Edit and a switch to turn it on or off.
- Rotate URL gives a webhook task a new URL. The current URL stops working.
- A device whose `maxd` predates scheduled tasks isn't listed.

## Test hosts: where test machines run
<!-- id: settings/test-hosts -->
<!-- covers: settings/test-hosts -->

Test hosts picks which devices run each kind of test machine for the agents there: iOS Simulator, Android emulator, Linux and macOS VM.

- Each kind lists the devices that could host it. Tick a device to host that kind there.
- Once ticked, it shows what that device's `maxd` found, with Build, Reset and Check again. A Mac can use its own Simulator instead.
- Only agents on a device lease its test machines. For an agent elsewhere to test on one, start an agent on the device that hosts it.
- When no device offers test hosts, the page says so. They come with a newer `maxd`.

## Skills: the library and global skills
<!-- id: settings/skills -->
<!-- covers: settings/skills -->

The Skills page holds the skill library: the sources projects pick skills from, the skills every agent gets, and a tidy for each device's global skills.

- Sources: the repos and folders the library holds, on one device. Each shows Yours or Third party, with Fetch, Remove, and Push for your own.
- Add a source: Repo or folder takes `owner/repo`, a git URL or a folder path. Name and Branch or tag are optional. Tick A third party's skills to pin them until you update them. Add source adds it.
- Every agent: the Skills every agent gets. They're global on each device, so projects can't take them.
- Global skills: Tidy global skills moves your own global skills into the library, device by device. Undo puts them back.
- A project picks its own skills on its Skills tab. See [Projects and workspaces](projects.md).

## Notifications: alerts, sounds and the Dock badge
<!-- id: settings/notifications -->
<!-- covers: settings/notifications -->

The Notifications page sets which alerts this Mac shows, which of them play a sound, and what the Dock badge counts.

- The top line says what macOS allows. When MaxHQ's notifications are off there, Open System Settings takes you to them.
- Show notifications: alerts from your agents, in Notification Center. Send a test notification shows one, whichever kinds are on. The first test makes macOS ask.
- Alerts, the same kinds as on your phones: Needs input (a permission, a question or a secret), Error (an agent stopped on an error), Finished (a turn ended while you were away) and Limit (an agent waits for a usage limit to reset).
- Sounds: Prompts plays one for an alert that waits on you, needs input or an error. Other alerts plays one for Finished and Limit.
- Dock badge: Show the badge puts the number of agents that need you on MaxHQ's icon. Each agent counts once. Pick what counts: Waiting on you, Finished, and Failed or stopped with an error.
- macOS hides the badge while MaxHQ's notifications or badges are off in System Settings.

## Updates: version and checking for updates
<!-- id: settings/updates -->
<!-- covers: settings/updates -->

The Updates page shows this app's version and where its update stands. Check now looks at once, and Update now installs a newer version.

- Version: the version this app runs, and a line on its update, such as when it last checked.
- Check now looks for a newer version. When one is there, What's new opens its notes and Update now installs it.
- Check for updates automatically: a little after MaxHQ opens, then every few hours. It's kept on this Mac.
- A copy that doesn't update itself says why. MaxHQ updates itself only from the Applications folder, so move it there. A copy built from source updates by building the new version.

## Phones: pair a phone
<!-- id: settings/phones -->
<!-- covers: settings/phones -->

The Phones page pairs a new phone with a code, and lists the phones that reach your devices. Add phone starts it.

1. Under Add phone, tick the Devices the phone can reach.
2. With more than one ticked, pick the one to Pair through.
3. Click Show the code. Scan the code with MaxHQ on the phone: on the phone, choose Scan the code. The pairing code works once, for 10 minutes. A new code doesn't cancel the one before.

- A device without the APNs key says so, since it can't push to the phone. Add the key file opens Phone pushes.
- Each paired phone shows the devices that let it in, with Remove.
- New code: for the devices a phone hasn't signed in to over the network yet. Open MaxHQ on the phone within 10 minutes.
- Let this phone do everything: off by default, and it asks first. Anyone holding the phone could then start agents that run commands on your devices, change settings, store or remove sign-ins, and copy files out. Adding keys or other phones still needs this Mac.
- Address for the phone: on each device's page, where a phone dials it, such as `devbox.example.net:22`. Left empty, the phone uses the one the Mac uses.

## Keyboard shortcuts page
<!-- id: settings/shortcuts -->
<!-- covers: settings/shortcuts -->

The Keyboard shortcuts page lists every key MaxHQ listens for. Show keyboard shortcuts, `⌘/`, shows the same list over any view.

The list has groups: General, Agents, Sidebar, Inbox, Panes, Find, Composer and Notes, and Other for a key that has no words yet. Keys marked Projects only work only in projects mode. Every key is on [Keyboard shortcuts](shortcuts.md).

## Claude in workspaces: send a setup-token
<!-- id: settings/claude -->
<!-- covers: settings/claude -->

Claude in workspaces sends a setup-token from a Mac login to the devices that host workspaces, so Claude Code runs in their containers. It's for personal use only.

1. Under Send a token, pick the Mac login's Account.
2. Run this in Terminal on the Mac: the page shows the command, such as `claude setup-token`, with a copy button.
3. Sign in in the browser with the same account.
4. Paste the token the terminal printed into Setup-token, tick the devices, and click Send.

- MaxHQ sends it once to each ticked device's `maxd`, then clears the field. The Mac keeps no copy.
- A device takes a token only once Take setup-tokens is on in its own settings. See [Devices and links](devices.md).
- Tokens devices hold: Forget deletes a token on its device.
- Sign in again: runs a login's own sign-in on the device that holds it. A logged out login's button says Sign in.

## Codex in workspaces: sign each device in
<!-- id: settings/codex -->
<!-- covers: settings/codex -->

Codex in workspaces signs each device that hosts workspaces in to Codex on its own. Under Sign in, click Continue with ChatGPT next to a device.

1. Click Continue with ChatGPT next to the device.
2. Your browser opens. Sign in there.
3. Repeat for each device. Each one keeps its own sign-in.

- Sessions devices hold: each device's Codex session. Sign out ends the session with OpenAI, then deletes it from the device.
- The session never leaves the device that made it, and containers never hold a token.
- Sign in again: runs a Codex login's own sign-in on the device that holds it.
- With no device that has Incus, there's nowhere to sign in yet.

## Cursor in workspaces: send an API key
<!-- id: settings/cursor -->
<!-- covers: settings/cursor -->

On the Mac, Cursor agents run as the Cursor CLI's own login. For containers, send a Cursor API key to the devices that host workspaces.

- On the Mac: shows the CLI's account. Use Sign in again, or run `agent login` in Terminal on the Mac once.
- Send an API key: create a user API key in the Cursor dashboard under Integrations, paste it into API key, give it a name if you like, tick the devices and click Send. The same key can go to every device.
- MaxHQ sends it once to each ticked device's `maxd`, then clears the field. The Mac keeps no copy.
- Keys devices hold: a device holds one key, so sending another replaces it. Forget deletes it on the device.
- Cursor shows no usage meters in MaxHQ. Manage usage opens the Cursor dashboard.

## GitHub App: pushes and # lookups
<!-- id: settings/github -->
<!-- covers: settings/github -->

The GitHub App lets workspaces push, and lets `#` in the composer find pull requests and issues. Make the App on GitHub, then send its App ID and private key to your devices.

The page's Steps card has these steps, with links and copy buttons. You do them once.

1. Open https://github.com/settings/apps/new, signed in to the account that owns your repos. For an organization's repos, open the organization's Settings, Developer settings, GitHub Apps instead.
2. GitHub App name: `MaxHQ-<your GitHub login>`. No other app may use the name.
3. Homepage URL: `https://github.com/<your GitHub login>`.
4. Leave Callback URL empty. Leave "Request user authorization (OAuth) during installation" and "Enable Device Flow" unticked.
5. Under Webhook, untick Active.
6. Under Repository permissions, set Contents, Issues and Pull requests to "Read and write". Metadata turns "Read-only" by itself. Leave every other permission at "No access".
7. Under "Where can this GitHub App be installed?", choose "Only on this account".
8. Click Create GitHub App. Copy the App ID from the top of the App's page into MaxHQ.
9. Under Private keys, click Generate a private key. The browser saves a file ending in `.private-key.pem`.
10. Under Send the App, choose or drop that file as the Private key, and tick the devices: each one that hosts workspaces, and any other whose agents look things up with `#`. Click Send. Then Move to Trash removes the downloaded file. The Mac keeps no copy.
11. On the App's GitHub page, click Install App, choose your account, choose "Only select repositories" and pick the repos your projects use. Add more later on the same page. MaxHQ finds the installation itself.
12. MaxHQ checks the setup. Each device lists the repos the App reaches.

- Apps devices hold: the repos each device's App reaches, with Check again and Forget.
- Workspaces never see the key. A device hands a session only a one-hour token for one repo of its project.
- An App made before MaxHQ asked for Issues still works, and the check says so. To add it, set Issues to "Read and write" on the App's Permissions & events page, then accept the request under Settings, Applications, Installed GitHub Apps, Configure.
- Merge and approve as you: Sign in to GitHub on one device, enter the code GitHub shows and press Authorize. Merges and approvals then go through your own sign-in. Sign out has that device forget it.
- A device without the App still finds pull requests and issues for `#` through `gh`, once `gh auth login` runs there.

## Phone pushes: the APNs key
<!-- id: settings/apns -->
<!-- covers: settings/apns -->

Phone pushes sends an Apple APNs key to each device, so it can push alerts to your phones. One key works for every device and every phone.

The page's Steps card has these steps, with links and copy buttons.

1. Open https://developer.apple.com/account/resources/authkeys/list, signed in to the Apple developer account the MaxHQ phone app is built with.
2. Click the plus button, name the key, such as `MaxHQ pushes`, tick Apple Push Notifications service (APNs), and click Continue, then Register.
3. Copy the Key ID from the key's page into MaxHQ. It's 10 letters and digits.
4. Click Download. The browser saves `AuthKey_<Key ID>.p8`. Apple lets you download it only once.
5. Copy your Team ID into MaxHQ. It's at the top right of the developer account, or under Membership details.
6. Under Send the key, choose or drop the `.p8` file as the Private key, tick the devices that push to phones and click Send. Then Move to Trash removes the downloaded file. The Mac keeps no copy.

- Push keys devices hold: a device holds one key, so sending another replaces it. Forget deletes it, and that device stops pushing.
- What a push says is sealed to the phone's own key, so Apple can't read it.
