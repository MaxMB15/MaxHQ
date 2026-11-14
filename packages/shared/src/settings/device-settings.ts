// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's settings as its `maxd` keeps them, with the defaults filled in
// for any it doesn't report. A save sends them all, so a client that
// changes one setting sends the rest as the device has them.

import type { DeviceId } from "../bindings";
import type { DeviceSettings } from "../fleet/wire";
import { testHostSettingsOf } from "../test-hosts/hosts";
import type { ViewState } from "../view/replica";
import { DEFAULT_IDLE_RELEASE_MINUTES } from "./idle-release";
import { DEFAULT_SETTLE_DAYS } from "./inbox";
import { DEFAULT_HISTORY_MIB, DEFAULT_MEMORY_ITEMS } from "./memory";
import { deviceDefault } from "./permission";

/** Every setting a save sends. */
export type DeviceFields = Required<Omit<DeviceSettings, "device">>;

/** A device's settings as it has them, with the defaults filled in. */
export const deviceSettingsOf = (view: ViewState, device: DeviceId): DeviceFields => {
	const kept = view.tables.device_settings.get(device);
	return {
		default_permission: deviceDefault(view, device),
		workspace_network: kept?.workspace_network ?? null,
		workspace_pool: kept?.workspace_pool ?? null,
		auto_settle_days: kept?.auto_settle_days ?? DEFAULT_SETTLE_DAYS,
		snooze_at_limit: kept?.snooze_at_limit ?? false,
		continue_claude_at_limit: kept?.continue_claude_at_limit ?? false,
		continue_codex_at_limit: kept?.continue_codex_at_limit ?? false,
		claude_effort: kept?.claude_effort ?? null,
		codex_effort: kept?.codex_effort ?? null,
		placement: kept?.placement ?? "normal",
		tool_updates: kept?.tool_updates ?? [],
		cursor_usage: kept?.cursor_usage ?? false,
		keep_awake: kept?.keep_awake ?? "charger",
		idle_release_minutes: kept?.idle_release_minutes ?? DEFAULT_IDLE_RELEASE_MINUTES,
		memory_items: kept?.memory_items ?? DEFAULT_MEMORY_ITEMS,
		history_mib: kept?.history_mib ?? DEFAULT_HISTORY_MIB,
		claude_setup_token: kept?.claude_setup_token ?? false,
		skill_library: kept?.skill_library ?? null,
		network: kept?.network ?? {},
		test_hosts: testHostSettingsOf(view, device),
	};
};
