// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Android's notification channels, one per push kind and named after its
// switch, so each can be changed in the system's settings. The push
// module shows each push in the channel of its kind. Prompts sound and
// show over the screen; the other kinds arrive quietly, as on an iPhone,
// where only prompts sound. iOS has no channels.

import * as Notifications from "expo-notifications";

import type { PushKind } from "@maxhq/shared/bindings";
import { PUSH_KINDS } from "@maxhq/shared/push/push";

import { platform } from "../platform";

/** The channel of the pushes that wait on you, which a button's stand-in note uses too. */
export const PROMPT_CHANNEL: PushKind = "needs_input";

/**
 * Makes the channels, or renames them in the language now in use. Android
 * 13 and later asks for the permission only once the app has a channel,
 * so this goes first.
 */
export const makeChannels = async (): Promise<void> => {
	if (!platform.notificationChannels) return;
	await Promise.all(
		PUSH_KINDS.map(({ kind, label, detail }) =>
			Notifications.setNotificationChannelAsync(kind, {
				name: label,
				description: detail,
				...(kind === PROMPT_CHANNEL
					? { importance: Notifications.AndroidImportance.HIGH }
					: {
							importance: Notifications.AndroidImportance.DEFAULT,
							sound: null,
							enableVibrate: false,
						}),
			})
		)
	);
};
