// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The cards of a device's settings that every device has, and the Mac's
// and Incus host's own, as the desktop shows them: the mode and effort new
// agents start at, the inbox, usage limits, placement, idle agents,
// memory, setup-tokens, sleep, Cursor usage, new workspaces and the skill
// library. Each saves all of the device's settings with its part changed.

import { useI18n } from "@maxhq/shared/i18n";

import type { PermissionMode } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import type { KeepAwake, Placement } from "@maxhq/shared/fleet/wire";
import { modeOptions } from "@maxhq/shared/new-agent/spec";
import { AUTO_QUESTION, whenModeAllowed } from "@maxhq/shared/settings/auto-mode";
import {
	KEEP_AWAKE_OPTIONS,
	nameOptions,
	PLACEMENT_OPTIONS,
} from "@maxhq/shared/settings/device-settings";
import { idleReleaseOptions } from "@maxhq/shared/settings/idle-release";
import { parseSettleDays } from "@maxhq/shared/settings/inbox";
import {
	historyOptions,
	memoryItemsOptions,
	NO_LIMIT_WARNING,
} from "@maxhq/shared/settings/memory";
import { knowsSetupTokens, setupTokenQuestion } from "@maxhq/shared/settings/setup-tokens";
import { skillLibrary } from "@maxhq/shared/skills/library";
import { deviceName } from "@maxhq/shared/view/select";

import { useServices } from "../../services";
import { Card } from "../../ui/Card";
import { confirm } from "../../ui/choose";
import { autoConsent } from "../agent/auto-consent";
import { EffortRows } from "./device-effort";
import { type CardProps, isOff, NumberRow, PartNote, PickRow, SwitchRow } from "./device-parts";

const askAuto = (yes: () => void) => confirm(AUTO_QUESTION, AUTO_QUESTION.action, yes);

export const NewAgentsCard = ({ view, device, lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { shown, save, noteFor } = saver;
	const effortOff = isOff(why.effort, lets);
	return (
		<Card
			title={t("New agents")}
			note={t("Auto on the Mac and Full access on remote devices by default")}
		>
			<PickRow
				label={t("Default mode")}
				sub={
					why.offline ??
					t(
						"New agents here start in this mode unless you pick another. Running agents keep theirs."
					)
				}
				value={shown.default_permission}
				options={modeOptions(undefined)}
				off={isOff(why.offline, lets)}
				note={noteFor("mode")}
				onPick={(picked) => {
					const mode = picked as PermissionMode;
					whenModeAllowed(
						mode,
						autoConsent,
						askAuto,
						() => void save("mode", { default_permission: mode })
					);
				}}
			/>
			<EffortRows
				view={view}
				device={device}
				why={why.effort}
				off={effortOff}
				shown={shown}
				onSave={(change) => void save("effort", change)}
			/>
			<PartNote note={noteFor("effort")} />
		</Card>
	);
};

export const InboxCard = ({ lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { shown, save, noteFor, setNote } = saver;
	const off = isOff(why.inbox, lets);
	return (
		<Card title={t("Inbox")} note={t("Idle agents settle after 5 days by default")}>
			<NumberRow
				label={t("Auto-settle after")}
				sub={
					why.inbox ??
					t(
						"Agents here with no activity for this many days settle on their own, and any new activity brings them back. 0 never settles them."
					)
				}
				value={shown.auto_settle_days}
				unit={t("days")}
				off={off}
				onCommit={(text) => {
					const parsed = parseSettleDays(text);
					if (parsed.ok) void save("inbox", { auto_settle_days: parsed.days });
					else setNote("inbox", { text: parsed.reason, error: true });
				}}
			/>
			<SwitchRow
				label={t("Snooze at the limit")}
				sub={
					why.inbox ??
					t(
						"Agents that stop at a usage limit snooze until the limit resets, then come back."
					)
				}
				value={shown.snooze_at_limit}
				off={off}
				onChange={(on) => void save("inbox", { snooze_at_limit: on })}
			/>
			<PartNote note={noteFor("inbox")} />
		</Card>
	);
};

export const LimitsCard = ({ lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { shown, save, noteFor } = saver;
	const off = isOff(why.limits, lets);
	return (
		<Card title={t("Usage limits")} note={t("Off by default")}>
			<SwitchRow
				label={t("Claude Code agents continue")}
				sub={
					why.limits ??
					t(
						"An agent that a usage limit stops mid-turn gets a message to carry on once the limit resets. This is the default; turn it on or off for one agent from its menu in the sidebar."
					)
				}
				value={shown.continue_claude_at_limit}
				off={off}
				onChange={(on) => void save("limits", { continue_claude_at_limit: on })}
			/>
			<SwitchRow
				label={t("Codex agents continue")}
				sub={
					why.limits ??
					t(
						"The same for Codex. Cursor agents wait for you, since Cursor doesn't say when a limit resets."
					)
				}
				value={shown.continue_codex_at_limit}
				off={off}
				onChange={(on) => void save("limits", { continue_codex_at_limit: on })}
			/>
			<PartNote note={noteFor("limits")} />
		</Card>
	);
};

export const PlacementCard = ({ name, lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { shown, save, noteFor } = saver;
	return (
		<Card title={t("Placement")} note={t("Normal by default")}>
			<PickRow
				label={t("New agents you don't place")}
				sub={
					why.offline ??
					t(
						"When more than one device could run a new agent, MaxHQ picks the one with the most free CPU and memory. Prefer makes {{name}} count twice.",
						{ name }
					)
				}
				value={shown.placement}
				options={PLACEMENT_OPTIONS}
				off={isOff(why.offline, lets)}
				note={noteFor("placement")}
				onPick={(placement) =>
					void save("placement", { placement: placement as Placement })
				}
			/>
		</Card>
	);
};

export const IdleCard = ({ lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { shown, save, noteFor } = saver;
	return (
		<Card title={t("Idle agents")} note={t("1 hour by default")}>
			<PickRow
				label={t("Stop idle CLIs after")}
				sub={
					why.idle ??
					t(
						"maxd stops the CLI of an agent idle this long to free its memory. The agent stays as it is and starts again with your next message."
					)
				}
				value={String(shown.idle_release_minutes)}
				options={idleReleaseOptions(shown.idle_release_minutes)}
				off={isOff(why.idle, lets)}
				note={noteFor("idle")}
				onPick={(minutes) => void save("idle", { idle_release_minutes: Number(minutes) })}
			/>
		</Card>
	);
};

export const MemoryCard = ({ lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { shown, save, noteFor } = saver;
	const off = isOff(why.memory, lets);
	return (
		<Card title={t("Memory")} note={t("256 items and 256 MB per agent by default")}>
			<PickRow
				label={t("Recent items kept in memory per agent")}
				sub={
					why.memory ??
					t(
						"maxd keeps this many of each agent's newest items in memory, and the rest on disk until a client scrolls back. More uses more memory per agent."
					)
				}
				value={String(shown.memory_items)}
				options={memoryItemsOptions(shown.memory_items)}
				off={off}
				note={noteFor("memory")}
				onPick={(items) => void save("memory", { memory_items: Number(items) })}
			/>
			<PickRow
				label={t("History kept per agent")}
				sub={
					why.memory ??
					t(
						"Past this, an agent's oldest items leave its timeline on this device. They stay in the agent's own history."
					)
				}
				warn={why.memory === null && shown.history_mib === 0 ? NO_LIMIT_WARNING : null}
				value={String(shown.history_mib)}
				options={historyOptions(shown.history_mib)}
				off={off}
				note={noteFor("history")}
				onPick={(mib) => void save("history", { history_mib: Number(mib) })}
			/>
		</Card>
	);
};

export const SetupTokensCard = ({ view, device, name, lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { shown, save, noteFor } = saver;
	// Turning setup-tokens on says what that means first.
	const take = (on: boolean) => {
		if (!on) {
			void save("tokens", { claude_setup_token: false });
			return;
		}
		const question = setupTokenQuestion(name);
		confirm(question, question.action, () => void save("tokens", { claude_setup_token: true }));
	};
	return (
		<Card title={t("Claude setup-tokens")} note={t("Off by default. For personal use")}>
			<SwitchRow
				label={t("Take setup-tokens")}
				sub={
					why.tokens ??
					t(
						"Stores the Claude setup-token you send here and gives it to Claude Code agents when they start. Without it, sign in with claude on this device."
					)
				}
				value={knowsSetupTokens(view, device) ? shown.claude_setup_token : true}
				off={isOff(why.tokens, lets)}
				onChange={take}
			/>
			<PartNote note={noteFor("tokens")} />
		</Card>
	);
};

/** Sleep and Cursor usage, which only a Mac has. */
export const MacCards = ({ lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { shown, save, noteFor } = saver;
	const off = isOff(why.offline, lets);
	return (
		<>
			<Card title={t("Sleep")} note={t("On the charger by default")}>
				<PickRow
					label={t("Keep this Mac awake")}
					sub={
						why.offline ??
						t(
							"A sleeping Mac stops maxd, so your phone, webhooks and scheduled tasks can't reach it. The display still turns off. Closing the lid still sleeps the Mac."
						)
					}
					value={shown.keep_awake}
					options={KEEP_AWAKE_OPTIONS}
					off={off}
					note={noteFor("awake")}
					onPick={(mode) => void save("awake", { keep_awake: mode as KeepAwake })}
				/>
			</Card>
			<Card
				title={t("Cursor usage")}
				note={t("Off by default. One Mac covers the whole account")}
			>
				<SwitchRow
					label={t("Enable Cursor usage")}
					sub={
						why.offline ??
						t(
							"maxd reads the Cursor CLI's saved login from the Keychain, and macOS asks you once to allow it. It then asks Cursor for this month's usage through an endpoint Cursor doesn't document, so it may stop working."
						)
					}
					value={shown.cursor_usage}
					off={off}
					onChange={(on) => void save("cursor", { cursor_usage: on })}
				/>
				<PartNote note={noteFor("cursor")} />
			</Card>
		</>
	);
};

/** The Incus network and storage pool new workspaces get, on a device with Incus. */
export const WorkspacesCard = ({ view, device, lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { shown, save, noteFor } = saver;
	const host = view.tables.device.get(device)?.workspace_host;
	if (!host) return null;
	const off = isOff(why.offline, lets);
	return (
		<Card
			title={t("New workspaces")}
			note={t("What Incus gives each container MaxHQ makes here")}
		>
			<PickRow
				label={t("Network")}
				sub={
					why.offline ??
					t(
						"The Incus network new workspaces join. Pick one with a guard that keeps containers off your network."
					)
				}
				value={shown.workspace_network ?? ""}
				options={nameOptions(host.networks)}
				off={off}
				onPick={(network) => void save("workspace", { workspace_network: network })}
			/>
			<PickRow
				label={t("Storage pool")}
				sub={why.offline ?? t("The Incus storage pool new workspaces live on.")}
				value={shown.workspace_pool ?? ""}
				options={nameOptions(host.pools)}
				off={off}
				note={noteFor("workspace")}
				onPick={(pool) => void save("workspace", { workspace_pool: pool })}
			/>
		</Card>
	);
};

/** Whether the device holds the skill library. Turning it on moves the library here. */
export const LibraryCard = ({ view, device, name, live, lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { source } = useServices();
	const { noteFor, setNote } = saver;
	const holder = skillLibrary(view)?.device ?? null;
	const here = holder === device;
	const move = async () => {
		setNote("library", { text: t("Moving"), error: false });
		try {
			const failure = failureOf(await source.moveSkillLibrary(device));
			setNote(
				"library",
				failure ? { text: failure, error: true } : { text: t("Moved"), error: false }
			);
		} catch {
			setNote("library", { text: t("Couldn't move it. Try again."), error: true });
		}
	};
	return (
		<Card title={t("Skill library")} note={t("One device holds it at a time")}>
			<SwitchRow
				label={t("Holds the skill library")}
				sub={
					here
						? t(
								"{{name}} holds the skill library. To move it, turn this on for another device.",
								{ name }
							)
						: (why.offline ??
							(holder
								? t(
										"{{deviceName}} holds it now. Turning this on moves it and its sources to {{name}}.",
										{ deviceName: deviceName(view, holder), name }
									)
								: t(
										"No device holds it yet. Turning this on makes {{name}} hold it.",
										{ name }
									)))
				}
				value={here}
				off={!live || here || !lets}
				onChange={(on) => {
					if (on) void move();
				}}
			/>
			<PartNote note={noteFor("library")} />
		</Card>
	);
};
