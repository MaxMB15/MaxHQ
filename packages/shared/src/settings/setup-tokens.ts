// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether a device takes Claude setup-tokens: stores the one you send it
// and gives it to Claude Code agents when they start there. It's meant for
// personal use and is off unless you turn it on in the device's settings.
// Signing in with `claude` on the device needs no token. A `maxd` from
// before this setting doesn't report it, and always takes them.

import { t } from "../i18n";

import type { DeviceId } from "../bindings";
import type { ViewState } from "../view/replica";

/** Whether a device's `maxd` has the setting, as its settings show. */
export const knowsSetupTokens = (view: ViewState, device: DeviceId): boolean =>
	view.tables.device_settings.get(device)?.claude_setup_token !== undefined;

/** Whether a device takes setup-tokens now. One whose `maxd` predates the setting does. */
export const takesSetupTokens = (view: ViewState, device: DeviceId): boolean =>
	view.tables.device_settings.get(device)?.claude_setup_token !== false;

/** The question asked before setup-tokens turn on for the device named `name`. */
export const setupTokenQuestion = (name: string) => ({
	title: t("Turn on setup-tokens for {{name}}?", { name }),
	detail: t(
		"This is for personal use. MaxHQ stores your Claude setup-token on {{name}} and gives it to Claude Code agents when they start there. The alternative is to sign in with claude on each device.",
		{ name }
	),
	action: t("Turn on"),
});

/** Why a device can't get a setup-token while it doesn't take them, as the English key to `t`. */
export const SETUP_TOKENS_OFF = "Setup-tokens are off. Turn them on in its settings";
