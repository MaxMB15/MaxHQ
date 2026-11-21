// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's settings as its `maxd` keeps them, with the defaults filled in
// for any it doesn't report. A save sends them all, so a client that
// changes one setting sends the rest as the device has them. Also what a
// device's settings page shares between apps: whether two sets are the
// same, the placement and keep-awake choices, and why a part can't change.

import { setsEffort } from "../agent-view/effort";
import type { DeviceId } from "../bindings";
import type { DeviceSettings } from "../fleet/wire";
import { t } from "../i18n";
import { localSettingsOf, sameLocalSettings } from "../local-models/models";
import { runsLocalModels } from "../local-models/select";
import type { Option } from "../new-agent/spec";
import { sameTestHostSettings, testHostSettingsOf } from "../test-hosts/hosts";
import type { ViewState } from "../view/replica";
import { DEFAULT_CHAT_GAP_MINUTES } from "./chat-gap";
import { knowsClaudeArtifacts } from "./claude-artifacts";
import { DEFAULT_IDLE_RELEASE_MINUTES, releasesIdle } from "./idle-release";
import { DEFAULT_SETTLE_DAYS, keepsInboxes } from "./inbox";
import { continuesAtLimit } from "./limits";
import { DEFAULT_HISTORY_MIB, DEFAULT_MEMORY_ITEMS, keepsMemoryLimits } from "./memory";
import { deviceDefault } from "./permission";
import { knowsReadRights } from "./read-rights";
import { knowsSetupTokens } from "./setup-tokens";
import { knowsShellIntegration } from "./shell-integration";

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
		shell_integration: kept?.shell_integration ?? true,
		read_other_agents: kept?.read_other_agents ?? true,
		claude_artifacts: kept?.claude_artifacts ?? true,
		dispatcher_acts: kept?.dispatcher_acts ?? true,
		chat_gap_minutes: kept?.chat_gap_minutes ?? DEFAULT_CHAT_GAP_MINUTES,
		local_models: localSettingsOf(kept?.local_models),
		skill_library: kept?.skill_library ?? null,
		network: kept?.network ?? {},
		test_hosts: testHostSettingsOf(view, device),
	};
};

/** Whether two sets of settings say the same, tool updates in any order. */
export const sameDeviceFields = (a: DeviceFields, b: DeviceFields): boolean =>
	a.default_permission === b.default_permission &&
	a.workspace_network === b.workspace_network &&
	a.workspace_pool === b.workspace_pool &&
	a.auto_settle_days === b.auto_settle_days &&
	a.snooze_at_limit === b.snooze_at_limit &&
	a.continue_claude_at_limit === b.continue_claude_at_limit &&
	a.continue_codex_at_limit === b.continue_codex_at_limit &&
	a.claude_effort === b.claude_effort &&
	a.codex_effort === b.codex_effort &&
	a.placement === b.placement &&
	a.cursor_usage === b.cursor_usage &&
	a.keep_awake === b.keep_awake &&
	a.idle_release_minutes === b.idle_release_minutes &&
	a.memory_items === b.memory_items &&
	a.history_mib === b.history_mib &&
	a.claude_setup_token === b.claude_setup_token &&
	a.shell_integration === b.shell_integration &&
	a.read_other_agents === b.read_other_agents &&
	a.claude_artifacts === b.claude_artifacts &&
	a.dispatcher_acts === b.dispatcher_acts &&
	a.chat_gap_minutes === b.chat_gap_minutes &&
	sameLocalSettings(a.local_models, b.local_models) &&
	a.skill_library === b.skill_library &&
	a.network.on === b.network.on &&
	a.network.port === b.network.port &&
	a.tool_updates.length === b.tool_updates.length &&
	a.tool_updates.every((tool) => b.tool_updates.includes(tool)) &&
	sameTestHostSettings(a.test_hosts, b.test_hosts);

/** How readily new agents you don't place go to the device. */
export const PLACEMENT_OPTIONS: readonly Option[] = [
	{
		value: "prefer",
		get label() {
			return t("Prefer");
		},
		get detail() {
			return t("Counts twice when MaxHQ weighs devices");
		},
	},
	{
		value: "normal",
		get label() {
			return t("Normal");
		},
	},
];

/** When a Mac's `maxd` keeps it awake. */
export const KEEP_AWAKE_OPTIONS: readonly Option[] = [
	{
		value: "charger",
		get label() {
			return t("On the charger");
		},
		get detail() {
			return t("Awake whenever it's plugged in. On battery, only while an agent is working.");
		},
	},
	{
		value: "always",
		get label() {
			return t("Always");
		},
		get detail() {
			return t(
				"Awake whenever maxd runs, on battery too. A Mac left unplugged runs until its battery is low."
			);
		},
	},
	{
		value: "working",
		get label() {
			return t("While agents work");
		},
		get detail() {
			return t("Awake only while an agent on this Mac is working.");
		},
	},
	{
		value: "off",
		get label() {
			return t("Off");
		},
		get detail() {
			return t("MaxHQ never keeps this Mac awake, even while an agent works.");
		},
	},
];

/** Names to pick from, such as a host's Incus networks, each its own label. */
export const nameOptions = (names: readonly string[]): Option[] =>
	names.map((name) => ({ value: name, label: name }));

/**
 * Why each part of a device's settings can't change now, or null where it
 * can. `offline` says the device doesn't answer, which keeps every part
 * from changing; the others add a `maxd` too old for that part.
 */
export type SettingsWhy = {
	readonly offline: string | null;
	readonly inbox: string | null;
	readonly limits: string | null;
	readonly idle: string | null;
	readonly memory: string | null;
	readonly tokens: string | null;
	readonly effort: string | null;
	readonly shell: string | null;
	readonly reads: string | null;
	/** Whether Claude agents may use their tools for claude.ai pages. */
	readonly artifacts: string | null;
	/** The local model settings, which need a `maxd` that runs local models. */
	readonly local: string | null;
};

/** Why each part of `device`'s settings can't change, given whether it answers. */
export const settingsWhy = (
	view: ViewState,
	device: DeviceId,
	name: string,
	live: boolean
): SettingsWhy => {
	const offline = live ? null : t("Connect to {{name}} to change it.", { name });
	const tooOld = (knows: boolean, these: boolean) =>
		offline ??
		(knows
			? null
			: these
				? t("{{name}}'s maxd is too old for these. Update it first.", { name })
				: t("{{name}}'s maxd is too old for this. Update it first.", { name }));
	return {
		offline,
		inbox: tooOld(keepsInboxes(view, device), true),
		limits: tooOld(continuesAtLimit(view, device), false),
		idle: tooOld(releasesIdle(view, device), false),
		memory: tooOld(keepsMemoryLimits(view, device), true),
		tokens:
			offline ??
			(knowsSetupTokens(view, device)
				? null
				: t(
						"{{name}}'s maxd is too old for this, and takes setup-tokens. Update it first.",
						{
							name,
						}
					)),
		effort: tooOld(setsEffort(view, device), false),
		shell: tooOld(knowsShellIntegration(view, device), false),
		reads: tooOld(knowsReadRights(view, device), false),
		artifacts: tooOld(knowsClaudeArtifacts(view, device), false),
		local: tooOld(runsLocalModels(view, device), true),
	};
};
