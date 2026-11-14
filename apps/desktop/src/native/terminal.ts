// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Opens Terminal on a host item's root fix. The backend takes the command
// from the device's own report, so the page only names the item.

import { t } from "@maxhq/shared/i18n";

import type { DeviceId } from "@maxhq/shared/bindings";
import { commands } from "@/data/tauri-commands";
import { failureOf } from "@/settings/result";

import { inApp } from "./platform";

/** Whether this page can open Terminal at all. */
export const opensTerminal = (): boolean => inApp();

/**
 * Opens Terminal with the root fix of `item` on `device`. Resolves with
 * why it didn't open, or null when it did.
 */
export const openRootFix = async (device: DeviceId, item: string): Promise<string | null> =>
	inApp()
		? failureOf(await commands.openRootFix(device, item))
		: t("Terminal opens from the app only");
