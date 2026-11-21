<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Updates

How to keep the Mac app, maxd on your other devices and their agent CLIs up to date, and how to see which versions you run.

## Update the Mac app
<!-- id: updates/app -->
<!-- covers: settings/updates -->

Settings › Updates shows the app's version and whether a newer one is out. Click Check now to look, and Update now to install it.

1. Open Settings and pick Updates.
2. Read the Version row. It says MaxHQ is up to date, or which version is available.
3. Click What's new to read about it, then Update now.

MaxHQ downloads the update, puts it in place and restarts. With Check for updates automatically on, it checks a little after it opens and then every few hours, and asks with a window you can answer with Update now or Later.

## Check for updates from the status bar
<!-- id: updates/status-bar -->

The thin bar at the bottom of the window shows MaxHQ's version and protocol, with a button beside them to check for an update or install one.

- Click the round arrow, Check for updates. It spins while it looks, then says "Up to date" for a moment, or "Couldn't check" with the reason on hover.
- When an update is out, the arrow turns into Update. Hover it for the version, and click it to open the update window, even after you picked Later there.
- A copy built from source can't update, so the button is greyed: "Updates come with release builds. This copy was built from source."

The button checks only when you click it. Check for updates automatically in Settings › Updates does the checks on a timer.

## Fix an app that won't update itself
<!-- id: updates/cant-update -->
<!-- covers: settings/updates -->

The Version row in Settings › Updates says why when MaxHQ can't update itself. Most often it runs from the wrong place.

- "Running from its disk image": drag MaxHQ to Applications and open it from there.
- "A temporary copy": macOS runs a copy because you opened it where it was downloaded. Move it to Applications and open it from there.
- "Only from the Applications folder": move it there.
- "Built from source": a copy you built yourself doesn't update. Build the new version the same way.

When a check fails, the row says why. Click Check now to try again.

## Update maxd on your other devices
<!-- id: updates/everything -->

When a device's maxd or an agent CLI is behind, the bottom of the sidebar shows what needs updating, such as "Update 2 devices". Click Update everything to update them all.

1. Click the summary to see what each device gets: maxd, Claude, Codex or Cursor.
2. Click Update everything.
3. Watch each device go through its steps, such as uploading and restarting, until it's done.
4. If one fails, click Retry, or Dismiss to close the list.

A device marked "doesn't check signatures" runs an older maxd that installs what it's sent. When the maxd would be one no release signed, MaxHQ asks first, and sends it only after Install anyway.

## Update an agent CLI on one device
<!-- id: updates/clis -->

A device's Agent CLIs card lists Claude, Codex and Cursor there with their versions. Click Update on one that's behind. Update everything updates them too.

1. Open the device's page from the Devices view of the sidebar.
2. Find Agent CLIs.
3. Click Update on a CLI with a newer version out.

A CLI you installed yourself shows Let MaxHQ update. Turn it on to let MaxHQ update it. When MaxHQ can't update one, the row says so, or gives the command to run yourself with Copy command.

## See which versions you run
<!-- id: updates/versions -->
<!-- covers: settings/updates -->

The app's version is on the Version row in Settings › Updates. Each device's maxd version shows on its page, once MaxHQ connects to it.

- The app: Settings › Updates, or "This app" in the sidebar's update list while one shows.
- maxd on a device: open its page from the Devices view. The connection card says which maxd answers.
- An agent CLI: the Agent CLIs card on the device's page.

When a device's maxd is older than the app, some features say they need a newer maxd. Update it with Update everything.
