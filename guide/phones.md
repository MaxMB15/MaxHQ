<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Phones

How to pair an iPhone with your devices, what the phone app does, how it reaches your devices, and how pushes reach the phone.

## Pair a phone
<!-- id: phones/pair -->
<!-- covers: settings/phones -->

Pair a phone from the Mac's Settings with a code you scan in MaxHQ on the phone. The code works once, for 10 minutes.

1. On the Mac, open Settings, then Phones.
2. Under Add phone, tick the Devices the phone can reach.
3. With more than one ticked, pick the one to Pair through.
4. Click Show the code. The Pairing code shows.
5. On the phone, open MaxHQ, type This phone's name and choose Scan the code. Without the camera, use Or paste the code, then Pair.

The phone then lists the devices it can reach. If the code ran out, the phone says "That pairing is over. Start again on the Mac." Click Show a new code. A new code doesn't cancel the one before, which still works until its 10 minutes end. A device lets at most four pairings wait at once; past that it says to try again in a few minutes.

## What the phone app does
<!-- id: phones/app -->

The phone app shows the same agents as the Mac, so you can follow and answer them away from your desk.

The app opens on Agents, the inbox, with the agents that need you first, then each project. Its header has Usage, Devices, More and Settings. More holds New agent, Pull requests, Scheduled tasks and Select.

- Tap an agent to read its conversation, answer its prompts and send messages. From there, open Details, Sub-agents, Ports and processes, or Move to… for another device.
- A project's menu has New agent, Notes, New note, Pull requests, Workflows, Scheduled tasks, Skills, Copy to another device…, Hide project and Project settings….
- Devices lists each device. Tap one for its page, with its workspaces, New workspace, test machine leases and licences to accept.
- Usage shows your plans' limits.
- Settings has This phone, Paired Macs, About, App, Logins, Projects, Device settings and Devices.

The phone reaches each device through maxd, the same service the Mac uses. Some of these need Let this phone do everything; see below.

## Let a phone reach a device over the network
<!-- id: phones/network-access -->
<!-- covers: settings/device/network -->

By default the phone reaches a device over SSH. Turn on Network access on a device's page to let phones on the same network or tailnet reach it directly, over a connection pinned at pairing.

1. Open the device's page and find the Network access card.
2. Turn on Network access. The Port is 7466 by default.
3. The card lists the Addresses for phones the device gives out.
4. A phone paired earlier shows Not signed in on that device in Settings, Phones. Click New code, then open MaxHQ on the phone within 10 minutes.

Each signed-in phone has Revoke on the card. The phone has to pair again to reach that device.

## Set the address a phone dials
<!-- id: phones/address -->
<!-- covers: settings/phones -->

When the phone reaches a device over SSH, it dials the same address the Mac uses. Set another one when the phone can't reach that address, such as a name only the Mac's SSH config knows.

1. Open the device's page and find Phones.
2. Under Address for the phone, type where a phone dials it, such as devbox.example.com:22.
3. Leave it empty to use the one the Mac uses.

## Get pushes on the phone
<!-- id: phones/pushes -->
<!-- covers: settings/apns -->

Each device pushes to your phone itself, with your own Apple APNs key. Until a device holds the key, it can't push to the phone.

1. On the Mac, open Settings, then Phone pushes.
2. Fill in the Key ID and Team ID, choose the Private key file, tick the devices and send it under Send the key.
3. Push keys devices hold lists which device holds which key.
4. On the phone, turn on Notifications in MaxHQ's settings. iOS asks once.

After pairing, the Phones page names any device without the key, with Add the key file. Every paired device sends pushes. What a push says is sealed to the phone's own key. See [Settings](settings.md).

## Choose which pushes arrive and answer them
<!-- id: phones/notifications -->

The phone's Notifications switches pick the kinds of push you get. Every kind is on until you turn it off.

- Needs input: a permission, a question or a secret. Open it to answer.
- Error: an agent stopped on an error.
- Finished: a turn ended while you were away.
- Limit: an agent waits for a usage limit to reset.

None arrive for the agent on screen. A permission push has a Deny button that answers without opening the app, once the phone is unlocked. To allow, open the push and answer in the app. If iOS keeps them off, the row says so, with Open Settings.

## Let a phone do everything
<!-- id: phones/everything -->
<!-- covers: settings/phones -->

Let this phone do everything is a switch on each phone's row in the Mac's Settings, Phones. It's off by default.

1. Open Settings, then Phones, and find the phone.
2. Turn on Let this phone do everything.
3. Read what it allows, then choose Let it do everything, or Keep it off.

With it on, anyone holding the phone could start agents that run commands on your devices, change settings, store or remove sign-ins, and copy files out. Adding keys or other phones still needs this Mac. It stays on until you turn it off there. A device that isn't connected gets it once it's back.

With it off, the phone can still follow your agents and act on the ones that run:

- Answer prompts, send messages, steer, interrupt, stop, rename and delete agents, and change an agent's model, effort and mode.
- Settle, snooze and pin in the inbox, read and write notes, and ask the dispatcher.
- Merge, approve and link pull requests, run, turn on or off and delete scheduled tasks, and start or stop workspaces.
- Change a project's settings, end test machine leases and accept licences.
- Run a ! command in an agent's terminal, watch it and stop it with Ctrl-C.

It can't start or move agents, save a scheduled task, change a device's settings, install anything, make, copy or delete workspaces, restore a snapshot, browse folders, open a shell, forward ports, stop processes, copy files out, build or reset test machines, change skills, or store sign-ins. Those show greyed out on the phone, and a device that's asked anyway answers "A phone can't do that. To let this phone, turn on Let this phone do everything for it in the Mac's Settings, under Phones."

## Remove a phone or pair it again
<!-- id: phones/remove -->
<!-- covers: settings/phones -->

Each phone on the Mac's Phones page shows the devices that let it in, with Remove and, where it needs one, New code.

- Remove takes the phone off every device. It has to pair again to reach them.
- Not signed in on a device means the phone hasn't signed in there over the network yet. Click New code, then open MaxHQ on the phone within 10 minutes.
- A phone can't be renamed, on the Mac or on the phone. Its name is the one typed in This phone's name when it paired, iPhone unless you changed it. The phone shows how your devices list it.

On the phone, Paired Macs has Forget, which makes the phone forget that Mac's devices and sign out of them.

## Use a phone with more than one Mac
<!-- id: phones/several-macs -->

A phone can pair with several Macs, each with its own devices.

1. On the phone, open Paired Macs and choose Pair with another Mac.
2. Scan the code from the other Mac's Settings, Phones. This code adds a Mac, and your other Macs stay paired.
3. Under Show your Macs, pick Together to mix every Mac's agents, or One at a time to show one Mac's agents, devices and cards.

Forget on one Mac leaves the others paired. A device that another Mac also lists stays.
