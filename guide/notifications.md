<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Notifications

How MaxHQ tells you an agent needs you: the bell in the title bar, alerts in Notification Center, sounds, the Dock badge, and pushes to your phones.

## See what needs you with the bell
<!-- id: notifications/bell -->

The bell in the title bar counts the agents that need you, the same ones the Dock badge counts. Click it to see them, and the notifications before them.

- Now lists the agents that need you, each with why, such as "Asking you a question" or "Reported an error". An agent waiting on you "Stays until you answer it", and one that failed "Stays until it runs again". Click a row to open the agent at its prompt.
- Earlier lists past notifications from every connected device, newest first. Unread ones have a dot. Show more loads the next 20. An agent you deleted reads "This agent is gone".
- Mark all as read marks every notification read, and Clear forgets the ones under Earlier.
- Notification settings opens Settings › Notifications.

Opening an agent anywhere marks its notifications read. Turn off Show the badge and the bell shows no count, but a click still opens it. ⌘J still opens the next agent waiting on you.

## Turn on notifications
<!-- id: notifications/turn-on -->
<!-- covers: settings/notifications -->

Turn on Show notifications in Settings › Notifications to get alerts from your agents in Notification Center. macOS has to allow them too.

1. Open Settings and pick Notifications.
2. Turn on Show notifications.
3. Click Send a test notification. The first time, macOS asks whether MaxHQ may notify you.
4. If the page says macOS turned them off, click Open System Settings and allow MaxHQ there.

The test shows one notification whichever alert kinds are on.

## Pick which alerts you get
<!-- id: notifications/alerts -->
<!-- covers: settings/notifications -->

The Alerts section of Settings › Notifications picks the kinds you get. They are the same kinds your phones get.

- Needs input: an agent asks for a permission, an answer or a secret. It alerts at once, unless that agent's pane has focus.
- Error: an agent stopped on an error.
- Finished: a turn ended while you were away.
- Limit: an agent waits for a usage limit to reset.

Finished and Limit alert only when no pane shows that agent. Each alert shows once and goes away when the agent's state clears. Click one to open the agent's pane.

## Choose alert sounds
<!-- id: notifications/sounds -->
<!-- covers: settings/notifications -->

Under Sounds in Settings › Notifications, Prompts plays a sound for alerts that wait on you, and Other alerts plays one for the rest.

- Prompts covers Needs input and Error. It's on by default.
- Other alerts covers Finished and Limit. It's off by default.

Sounds follow the alert kinds you picked, so a kind that's off makes no sound either.

## Show a count on the bell and the Dock icon
<!-- id: notifications/dock-badge -->
<!-- covers: settings/notifications shortcut/open-the-next-agent-waiting-on-you -->

Show the badge, under Badge in Settings › Notifications, puts how many agents need you on the bell and on MaxHQ's Dock icon. It's on at first. Turn it off and neither shows a count, though the bell still opens its lists. Each agent counts once.

Pick what counts, for the bell and the Dock badge alike:

- Waiting on you: a permission, a question or a consent card.
- Finished: a turn ended and you haven't opened it.
- Failed or stopped with an error.

macOS hides the Dock badge when notifications or badges for MaxHQ are off in System Settings, and the bell still counts. To work through the agents the badge counts, press ⌘J to open the next agent waiting on you.

## Get alerts on your phone
<!-- id: notifications/phone -->

Every paired device pushes the same alert kinds to your phones, so you hear about an agent with your Mac closed. The pushes need a key set up once, in Settings › Phone pushes.

1. Pair the phone in Settings › Phones.
2. Set up the key in Settings › Phone pushes. Each device then holds it and sends pushes itself.
3. On the phone, open Settings › Notifications and tap Turn on. iOS asks once.
4. Pick the kinds the phone gets on the same page.

No push arrives for the agent the phone has on screen.
