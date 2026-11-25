<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Settings

What Settings holds, where to find each setting, and what its labels do. Settings is one page of groups, with a tab for each device, and a few views of their own.

## Open the settings
<!-- id: settings/open -->
<!-- covers: view/settings -->

Click the gear at the top of the window, or press `⌘,` (`Ctrl+,` off a Mac). Settings opens where you left it.

- The left column lists the groups: General, Chat and agents, Notifications, Accounts, Devices and phones, Permissions and Projects. Click one to scroll the page to it. The group you're in is marked as you scroll.
- Under Their own views are Skills, Scheduled tasks, Local models and Keyboard shortcuts. Skills, Scheduled tasks and Local models open in place of the page, with Back at the top. Keyboard shortcuts shows its list over any view.
- A login's Manage and a project's Open also open a view in place of the page. Back, or Esc, returns to the page where you were. A second Esc closes Settings.
- Type in Search settings, or press `⌘F` while Settings is open, to show only the cards whose titles or labels have your words. Each match is marked, groups with no match hide, and the left column lists the matches under their groups.
- A match on a device's tab lists the device's name. Pick it to open that device's tab at the card. Enter opens the first match.
- Esc clears the search first, and closes Settings once the search is empty.

## Where each setting is kept
<!-- id: settings/where-kept -->

Each card has a tag that says where it applies.

- This Mac: the card saves on this Mac only. Appearance, Updates, the Chat and agents cards, Notifications, Badge and Remembered answers are kept on this Mac.
- Every device: the card covers all your devices, with a row for each one where it differs. The accounts table, Cursor usage, Other agents, Grants, New permissions, Access keys, App keys and the projects list cover every device.
- A device's name: the card is on that device's tab under Devices and phones, and a change goes to that device only. See [Devices and links](devices.md).
- A project's settings and its skills open from the projects list. See [Projects and workspaces](projects.md).

## Appearance: theme and language
<!-- id: settings/appearance -->
<!-- covers: settings/appearance -->

The Appearance card, under General, sets the app's colors and the language of its interface. Both are kept on this Mac and apply at once.

- Colors, Theme: System, Light or Dark. System follows the Mac's appearance as it changes through the day.
- Language, Interface language: System, English, Nederlands, Deutsch, Français or Español. System follows this device's language.

## Sidebar: Working shelf and snooze options
<!-- id: settings/sidebar -->
<!-- covers: settings/sidebar -->

The Sidebar card, under Chat and agents, sets how the sidebar lists agents: whether busy agents fold away, and the choices in every snooze menu. It's kept on this Mac.

- Shelves, Working shelf: turn it on to fold busy agents into a Working shelf. They come back to the top when they need you.
- Snooze options: the choices in every snooze menu, listed soonest first. Each one is In an amount of minutes, hours or days, or Today, Tomorrow or a weekday at a time. A weekday means the next one after today.
- Add option adds one, up to 8. The cross next to an option removes it. Reset to defaults brings back the starting list.

## Composer, Open in and Above the conversation: spelling, editor and port prompts
<!-- id: settings/editor -->
<!-- covers: settings/composer settings/open-in settings/above-conversation -->

Three cards under Chat and agents set how you type to agents, where files open, and what shows above an agent's conversation. They're kept on this Mac.

- Composer, Typing, Check spelling: underlines misspelled words in the composer and in notes, with the Mac's guesses on a right click.
- Correct spelling automatically: lets macOS fix words, curl quotes and expand text replacements as you type. It needs Check spelling.
- While an agent works, Enter: Queue waits until the agent finishes, then sends. Steer sends at once, and the agent reads it between steps. `⌘⏎` does the other one.
- Open in, Editor, Default editor: the editor the Open in button and the buttons on file rows use. Pick Last used, Cursor, VS Code or None. Last used is the one you opened last. None hides those buttons. An editor this Mac doesn't have says "not found".
- Above the conversation, Conversation, Port prompts: offers to forward or open a port an agent starts listening on, at the top of its view. Off, ports still show in each agent's header. See [Ports and forwards](ports.md).

## Dispatcher: its account and whether it acts directly
<!-- id: settings/dispatcher -->
<!-- covers: settings/dispatcher -->

The Dispatcher card, under Chat and agents, picks which Claude or Codex account answers the box at the top of the window, and whether the dispatcher makes changes without asking. It's kept on this Mac.

- Account, Runs as: pick one of this Mac's Claude or Codex accounts. Without a pick, the dispatcher takes the first Claude account, else the first Codex one.
- With no account on this Mac, the card says to sign in to Claude or Codex first.
- Changes, Act directly: the dispatcher changes settings, agents and tasks without asking first. Deleting, stopping busy agents and granting permissions still ask. Off, every change waits in a plan you confirm. See [The dispatcher](dispatcher.md).
- Chats, New chat after a pause: when you write again after this long, the dispatcher starts a fresh chat. Pick 15 minutes, 30 minutes, 1 hour, 2 hours, 4 hours or Never. It starts at 30 minutes. It still goes back to an earlier chat on the same topic.
- A Mac whose maxd is too old for a setting greys it out and says to update maxd first.
- The dispatcher reads your projects and agents, and can't change your files.

## Accounts: the logins on each device
<!-- id: settings/accounts -->
<!-- covers: settings/accounts settings/cursor-usage -->

The Accounts group has a table with a row for each login agents use, Claude, Codex, Cursor and GitHub, and a column for each linked device.

- Each cell says Signed in, Not signed in or Not installed, for that login on that device.
- Manage opens the login's own view, with its steps and what each device holds. When a device still needs a sign in, the button says Sign in on that device's name, such as Sign in on devbox, and opens the view with that device picked.
- The Apple push key is on the Phones tab under Devices and phones.
- Cursor usage, below the table: a row for each Mac with Enable Cursor usage, off by default. One Mac covers the whole account. macOS asks once to let maxd read the Cursor CLI's saved login from the Keychain. maxd then asks Cursor for this month's usage through an endpoint Cursor doesn't document, so it may stop working.

## Permissions: between agents and outside tools
<!-- id: settings/agent-tools -->
<!-- covers: settings/other-agents settings/grants settings/new-permissions settings/claude-artifacts settings/access-keys settings/app-keys settings/remembered-answers -->

The Permissions group sets what agents may do to each other, and what tools outside MaxHQ may do on your devices. Between agents comes first, then Outside tools.

- Other agents: a row for each device with Agents can read other agents, on by default. Off, agents there read only the agents you attach or quote. Starting, messaging or stopping agents always needs your permission.
- Grants: Agents allowed to run agents lists each agent you allowed, with Revoke. Allow one from its agent's menu, with Allow to run agents.
- New permissions: Depth, Agents started, Agents at once, Usage cap and Expires after, plus the Devices, Vendors, Accounts, Places and Test hosts a new permission starts with. They start 3 levels deep with no limits. An empty limit is off. Save defaults saves them.
- Depth counts the levels of agents below the one that holds the permission. Limits count agents and usage. They don't fence off files: an agent that should stay away from your files runs in a container, with its permission's Containers setting on Ask or Never.
- claude.ai pages: a row for each device with Claude agents can use claude.ai artifacts, on by default. Off removes Claude Code's Artifacts tools from Claude agents there once their CLI next starts. See [Permissions](permissions.md).
- Access keys, For tools outside MaxHQ, such as a bot: New access key takes a Key name and the device it's kept on, then what it may use and its limits. Make key makes it. Each key shows where its file is, Copy MCP command for the command a tool runs, and Revoke. The page never shows the key itself.
- App keys: the apps each device trusts to do everything there, one row per key, with Remove. This Mac's app key shows the key this Mac's app signs with. Make a new app key moves every device to a new key for this Mac, and asks first.
- Remembered answers: the answers this Mac keeps to the questions that ask for your consent. Auto mode says whether you confirmed it. Forget makes picking Auto mode ask again.

See [Permissions](permissions.md).

## Devices and phones: a tab for each device
<!-- id: settings/devices -->
<!-- covers: settings/device/connection -->

The Devices and phones group has a tab for each linked device and one for Phones. It opens on this Mac's tab.

- Connection, at the top of a device's tab: how this Mac reaches it (Reached by), the maxd it runs, its System and its Device ID, plus its protocol and build when the device says them. Select a value to copy part of it, or hover it for a copy button.
- Address for the phone: where a phone dials the device, such as `devbox.example.net:22`. Left empty, the phone uses the one the Mac uses.
- Below Connection come the device's cards: New agents, Inbox, Resources, Terminals, Claude setup-tokens, and on a device that has them, New workspaces, Skill library, Network access and Test machines. See [Devices and links](devices.md).
- A device's page has Settings, which opens its tab here. A link to one of its cards opens the tab at that card.
- The Phones tab has Phones and Phone pushes.

## Projects: each project's settings
<!-- id: settings/projects -->
<!-- covers: settings/projects -->

The Projects group lists your projects, each with its checkout's folder and the devices that hold one. Open shows the project's settings in place of the page, with General and Skills tabs. See [Projects and workspaces](projects.md).

## Scheduled tasks: prompts on a timer or a webhook
<!-- id: settings/scheduled-tasks -->
<!-- covers: settings/scheduled-tasks -->

Scheduled tasks opens as a view of its own, from the left column. It lists a device's tasks: prompts a device sends to an agent on a timer, on a webhook or on GitHub events. New task makes one.

- With more than one device, the row at the top picks the device whose tasks show.
- New task opens the form: a Name, the Prompt, When it runs (Every some minutes, At a time on chosen days, Webhook or GitHub events), and Where it goes (Sends to and Lives on).
- A webhook task has a Webhook part: Take requests turns it on or off, and Requests it takes says which requests start a run. The URL shows once, after you save.
- Each task shows when it runs next and how it last went, with History, Run now, Edit and a switch to turn it on or off.
- Rotate URL gives a webhook task a new URL. The current URL stops working.
- A device whose `maxd` predates scheduled tasks isn't listed.

## Test machines: where test machines run
<!-- id: settings/test-hosts -->
<!-- covers: settings/device/test-machines -->

The Test machines card, on a device's tab, picks which kinds of test machine that device hosts for its agents: iOS Simulator, Android emulator, Linux and macOS VM. It shows on a device whose `maxd` offers test hosts.

- Each kind the device could host has a box. Tick it to host that kind there.
- Once ticked, it shows what the device's `maxd` found, with Build and Reset. Reset ends a lease on it. A Mac can use its own Simulator instead.
- Lease length is how long a lease lasts when the agent doesn't say, 30 minutes by default. Longest lease is the most an agent may ask for, 240 minutes by default. Ends when idle ends a lease after this long without a command, 10 minutes by default.
- On a Mac, macOS VMs at once sets how many VMs run together. Apple lets a Mac run two.
- Check again asks the device's `maxd` to look again.
- Only agents on a device lease its test machines. For an agent elsewhere to test on one, start an agent on the device that hosts it.
- The leases agents hold now show on the device's page, under Leases.

## Skills: the library and global skills
<!-- id: settings/skills -->
<!-- covers: settings/skills -->

Skills opens as a view of its own, from the left column. It holds the skill library: the sources projects pick skills from, the skills every agent gets, and a tidy for each device's global skills.

- Sources: the repos and folders the library holds, on one device. Each shows Yours or Third party, with Fetch, Remove, and Push for your own.
- Add a source: Repo or folder takes `owner/repo`, a git URL or a folder path. Name and Branch or tag are optional. Tick A third party's skills to pin them until you update them. Add source adds it.
- Every agent: the Skills every agent gets. They're global on each device, so projects can't take them.
- Global skills: Tidy global skills moves your own global skills into the library, device by device. Undo puts them back.
- A project picks its own skills on its Skills tab. See [Projects and workspaces](projects.md).

## Notifications: alerts, sounds and the badge
<!-- id: settings/notifications -->
<!-- covers: settings/notifications settings/badge -->

The Notifications group sets which alerts this Mac shows, which of them play a sound, and what the badge on the bell and the Dock badge count.

- The top line says what macOS allows. When MaxHQ's notifications are off there, Open System Settings takes you to them.
- Show notifications: alerts from your agents, in Notification Center. Send a test notification shows one, whichever kinds are on. The first test makes macOS ask.
- Alerts, the same kinds as on your phones: Needs input (a permission, a question or a secret), Error (an agent stopped on an error), Finished (a turn ended while you were away) and Limit (an agent waits for a usage limit to reset).
- Sounds: Prompts plays one for an alert that waits on you, needs input or an error. Other alerts plays one for Finished and Limit.
- Badge: Show the badge puts the number of agents that need you on the bell and on MaxHQ's icon in the Dock. Each agent counts once. Pick what counts: Waiting on you, Finished, and Failed or stopped with an error.
- macOS hides the Dock badge while MaxHQ's notifications or badges are off in System Settings. The bell still counts.

## Updates: version and checking for updates
<!-- id: settings/updates -->
<!-- covers: settings/updates -->

The Updates card, under General, shows this app's version and where its update stands. Check now looks at once, and Update now installs a newer version.

- Version: the version this app runs, and a line on its update, such as when it last checked.
- Check now looks for a newer version. When one is there, What's new opens its notes and Update now installs it.
- Check for updates automatically: a little after MaxHQ opens, then every few hours. It's kept on this Mac.
- A copy that doesn't update itself says why. MaxHQ updates itself only from the Applications folder, so move it there. A copy built from source updates by building the new version.

## Phones: pair a phone
<!-- id: settings/phones -->
<!-- covers: settings/phones -->

The Phones card, on the Phones tab under Devices and phones, pairs a new phone with a code, and lists the phones that reach your devices. Add phone starts it.

1. Under Add phone, tick the Devices the phone can reach.
2. With more than one ticked, pick the one to Pair through.
3. Click Show the code. Scan the code with MaxHQ on the phone: on the phone, choose Scan the code. The pairing code works once, for 10 minutes. A new code doesn't cancel the one before.

- A device without the APNs key says so, since it can't push to the phone. Add the key file goes to Phone pushes.
- Each paired phone shows the devices that let it in, with Remove.
- New code: for the devices a phone hasn't signed in to over the network yet. Open MaxHQ on the phone within 10 minutes.
- Let this phone do everything: off by default, and it asks first. Anyone holding the phone could then start agents that run commands on your devices, change settings, store or remove sign-ins, and copy files out. Adding keys or other phones still needs this Mac.
- Address for the phone: in each device's Connection card, where a phone dials it, such as `devbox.example.net:22`. Left empty, the phone uses the one the Mac uses.

## Keyboard shortcuts page
<!-- id: settings/shortcuts -->
<!-- covers: settings/shortcuts -->

Keyboard shortcuts, under Their own views, shows every key MaxHQ listens for over any view. Show keyboard shortcuts, `⌘/`, shows the same list.

The list has groups: General, Agents, Sidebar, Inbox, Panes, Find, Composer and Notes, and Other for a key that has no words yet. Keys marked Projects only work only in projects mode. Every key is on [Keyboard shortcuts](shortcuts.md).

## Claude in workspaces: send a setup-token
<!-- id: settings/claude -->
<!-- covers: settings/claude -->

Claude in workspaces sends a setup-token from a Mac login to the devices that host workspaces, so Claude Code runs in their containers. It's for personal use only. Open it with Manage on the Claude row under Accounts.

1. Under Send a token, pick the Mac login's Account.
2. Run this in Terminal on the Mac: the view shows the command, such as `claude setup-token`, with a copy button.
3. Sign in in the browser with the same account.
4. Paste the token the terminal printed into Setup-token, tick the devices, and click Send.

- MaxHQ sends it once to each ticked device's `maxd`, then clears the field. The Mac keeps no copy.
- A device takes a token only once Take setup-tokens is on in its Claude setup-tokens card. See [Devices and links](devices.md).
- Tokens devices hold: Forget deletes a token on its device.
- Sign in again: runs a login's own sign-in on the device that holds it. A logged out login's button says Sign in.

## Codex in workspaces: sign each device in
<!-- id: settings/codex -->
<!-- covers: settings/codex -->

Codex in workspaces signs each device that hosts workspaces in to Codex on its own. Open it from the Codex row under Accounts, then under Sign in, click Continue with ChatGPT next to a device.

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

On the Mac, Cursor agents run as the Cursor CLI's own login. For containers, send a Cursor API key to the devices that host workspaces. Open it from the Cursor row under Accounts.

- On the Mac: shows the CLI's account. Use Sign in again, or run `agent login` in Terminal on the Mac once.
- Send an API key: create a user API key in the Cursor dashboard under Integrations, paste it into API key, give it a name if you like, tick the devices and click Send. The same key can go to every device.
- MaxHQ sends it once to each ticked device's `maxd`, then clears the field. The Mac keeps no copy.
- Keys devices hold: a device holds one key, so sending another replaces it. Forget deletes it on the device.
- Shell rules Cursor saved: Always allow in an earlier MaxHQ also saved the rule in Cursor's own settings, and Cursor then runs commands that start with those words without asking MaxHQ, in every project on that device. Each device with Cursor lists them. Remove makes Cursor ask again; agents already running keep the rule until they start again. MaxHQ now keeps Always allow rules itself.
- Cursor usage, under Accounts, can show the account's usage. Manage usage opens the Cursor dashboard.

## GitHub App: pushes and # lookups
<!-- id: settings/github -->
<!-- covers: settings/github -->

The GitHub App lets workspaces push, and lets `#` in the composer find pull requests and issues. Make the App on GitHub, then send its App ID and private key to your devices. Open it from the GitHub row under Accounts.

The view's Steps card has these steps, with links and copy buttons. You do them once.

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

Phone pushes, on the Phones tab under Devices and phones, sends an Apple APNs key to each device, so it can push alerts to your phones. One key works for every device and every phone.

The card's steps have links and copy buttons.

1. Open https://developer.apple.com/account/resources/authkeys/list, signed in to the Apple developer account the MaxHQ phone app is built with.
2. Click the plus button, name the key, such as `MaxHQ pushes`, tick Apple Push Notifications service (APNs), and click Continue, then Register.
3. Copy the Key ID from the key's page into MaxHQ. It's 10 letters and digits.
4. Click Download. The browser saves `AuthKey_<Key ID>.p8`. Apple lets you download it only once.
5. Copy your Team ID into MaxHQ. It's at the top right of the developer account, or under Membership details.
6. Under Send the key, choose or drop the `.p8` file as the Private key, tick the devices that push to phones and click Send. Then Move to Trash removes the downloaded file. The Mac keeps no copy.

- Push keys devices hold: a device holds one key, so sending another replaces it. Forget deletes it, and that device stops pushing.
- What a push says is sealed to the phone's own key, so Apple can't read it.

## Android pushes: the Firebase key
<!-- id: settings/fcm -->
<!-- covers: settings/fcm -->

Android pushes, on the Phones tab under Devices and phones below Phone pushes, sends a Firebase service account key to each device, so it can push alerts to your Android phones through Firebase Cloud Messaging. One key works for every device and every Android phone. A device needs a `maxd` that knows Android pushes; an older one says to update it first.

1. Open https://console.firebase.google.com, signed in to the Google account that owns the Firebase project the MaxHQ Android app is built with, and open that project.
2. Click the gear beside Project Overview, choose Project settings, and open the Service accounts tab.
3. Click Generate new private key, then Generate key. The browser saves a JSON file named like `<project>-firebase-adminsdk-<id>.json`. Firebase shows the key only this once.
4. Under Send the Firebase key, choose or drop the JSON file as the Service account key, tick the devices that push to phones and click Send. Then Move to Trash removes the downloaded file. The Mac keeps no copy.

- Firebase keys devices hold: each line names the key's project and service account. A device holds one Firebase key, so sending another replaces it. Forget deletes it, and that device stops pushing to Android phones.
- To revoke the key itself, delete it under the service account's Keys in the Google Cloud console.
- What a push says is sealed to the phone's own key, so Google can't read it.
