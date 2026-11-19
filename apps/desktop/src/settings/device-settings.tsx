// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's settings in its detail: the mode new agents on it start in,
// when its idle agents settle and whether agents that stop at a usage limit
// snooze, how readily new agents go there when you don't pick a device,
// how long an idle agent keeps its CLI running, how much of each agent's
// timeline its `maxd` keeps, whether its terminals mark commands, whether
// it takes Claude setup-tokens, and,
// on a Mac, when its `maxd` keeps it awake, and, on a device with Incus,
// the network and storage pool new workspaces get, and whether phones reach
// it over the network. Its `maxd` keeps them and reports them, and a save
// sends them all, so changing one keeps the others as the device has them.
// Whether it holds the skill library is a move of its own, since only one
// device holds it. The phones signed in over the network each have a Revoke.
// The ACP agents set up there, the device links and the address for the phone
// follow, each saved on its own. Settings search finds the cards by their
// headings and labels, and opens the page scrolled to the one it picked.

import { type KeyboardEvent, useEffect, useState } from "react";

import { useServices } from "@/app/services";
import { requestConfirm } from "@/components/confirm";
import { Switch } from "@/components/ui/switch";
import type { DeviceId, PermissionMode } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import type { KeepAwake, Placement } from "@maxhq/shared/fleet/wire";
import { Picker } from "@/new-agent/picker";
import { modeOptions } from "@maxhq/shared/new-agent/spec";
import type { ViewState } from "@maxhq/shared/view/replica";
import { cn } from "@/lib/utils";
import { parseSettleDays } from "@maxhq/shared/settings/inbox";
import { idleReleaseOptions } from "@maxhq/shared/settings/idle-release";
import {
	historyOptions,
	memoryItemsOptions,
	NO_LIMIT_WARNING,
} from "@maxhq/shared/settings/memory";
import {
	DEFAULT_PORT,
	parsePort,
	sessionLine,
	signedIn,
	type SignedIn,
} from "@maxhq/shared/settings/network-access";
import {
	type DeviceFields as Fields,
	deviceSettingsOf,
	KEEP_AWAKE_OPTIONS,
	nameOptions,
	PLACEMENT_OPTIONS,
	sameDeviceFields as same,
	settingsWhy,
} from "@maxhq/shared/settings/device-settings";
import { knowsSetupTokens, setupTokenQuestion } from "@maxhq/shared/settings/setup-tokens";
import { knowsShellIntegration } from "@maxhq/shared/settings/shell-integration";
import { offersSkills, skillLibrary } from "@maxhq/shared/skills/library";
import { isThisMac, listsAppKeys } from "@maxhq/shared/settings/app-keys";
import { deviceName } from "@maxhq/shared/view/select";

import { AcpAgents } from "./acp-agents";
import { AppKeys } from "./app-keys";
import { pickMode } from "./auto-consent";
import { ConfirmButton, FIELD, Row, Section, StatusNote } from "./controls";
import { DeviceLinks } from "./device-links";
import { EffortDefaultRows } from "./effort-defaults";
import { PhoneAddress } from "./phones-settings";
import { failureOf, type Note } from "./result";
import { revealed, useSettings } from "./store";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

type Part =
	| "mode"
	| "effort"
	| "inbox"
	| "limits"
	| "placement"
	| "idle"
	| "memory"
	| "history"
	| "workspace"
	| "cursor"
	| "awake"
	| "library"
	| "network"
	| "shell"
	| "tokens";

/**
 * A number field. What you type stays here until you leave the field or
 * press Enter, so a half-typed number never reaches the device.
 */
const NumberField = ({
	label,
	value,
	unit,
	disabled,
	onCommit,
}: {
	label: string;
	value: number;
	/** What follows the field, such as "days". */
	unit?: string;
	disabled: boolean;
	/** Takes what you typed, once you're done with it. */
	onCommit: (text: string) => void;
}) => {
	useI18n();
	const [typed, setTyped] = useState<{ text: string; from: number } | null>(null);
	// Typing from before a save elsewhere is dropped, not saved over it.
	const draft = typed && typed.from === value ? typed.text : null;
	const commit = () => {
		setTyped(null);
		if (draft !== null) onCommit(draft);
	};
	return (
		<span className="flex items-center gap-2 text-xs text-muted-foreground">
			<input
				aria-label={label}
				inputMode="numeric"
				value={draft ?? String(value)}
				onChange={(e) => setTyped({ text: e.target.value, from: value })}
				onBlur={commit}
				onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
					if (e.key === "Enter") commit();
				}}
				spellCheck={false}
				autoComplete="off"
				disabled={disabled}
				className={cn(FIELD, "w-[64px] text-right tabular-nums")}
			/>
			{unit}
		</span>
	);
};

/** A phone signed in to the device over the network, with its Revoke. */
const PhoneSessionRow = ({ signed }: { signed: SignedIn }) => {
	useI18n();
	const { source } = useServices();
	const now = useNow(60_000);
	const [note, setNote] = useState<Note | null>(null);
	const revoke = async () => {
		try {
			const failure = failureOf(await source.revokePhoneSession(signed.session.id));
			if (failure) setNote({ text: failure, error: true });
		} catch (error) {
			console.warn(translate("Couldn't revoke the phone's sign-in"), error);
			setNote({ text: translate("Couldn't revoke it. Try again."), error: true });
		}
	};
	return (
		<Row title={signed.name} sub={sessionLine(signed.session, now)}>
			<StatusNote note={note} />
			<ConfirmButton
				label={translate("Revoke")}
				question="The phone has to pair again to reach it."
				confirm={translate("Revoke")}
				onConfirm={() => void revoke()}
			/>
		</Row>
	);
};

/** The settings sections for one device. */
/** A card on a device's page, as Settings search sees it. */
export type DeviceCard = {
	/** Names the card, for {@link cardId}. */
	readonly id: string;
	/** Its heading, then its controls' labels. */
	readonly terms: readonly string[];
};

/** The element ID of a device's card. Only one device's page shows at a time. */
export const cardId = (card: string): string => `device-card-${card}`;

/** The cards {@link DeviceSettings} shows for `device`, in order. */
export const deviceCards = (view: ViewState, device: DeviceId): readonly DeviceCard[] => {
	const record = view.tables.device.get(device);
	const cards: DeviceCard[] = [
		{
			id: "new-agents",
			terms: [translate("New agents"), translate("Default mode"), translate("Effort")],
		},
		{
			id: "inbox",
			terms: [
				translate("Inbox"),
				translate("Auto-settle after"),
				translate("Snooze at the limit"),
			],
		},
		{ id: "limits", terms: [translate("Usage limits"), translate("Continue after the limit")] },
		{
			id: "placement",
			terms: [translate("Placement"), translate("New agents you don't place")],
		},
		{ id: "idle", terms: [translate("Idle agents"), translate("Stop idle CLIs after")] },
		{
			id: "memory",
			terms: [
				translate("Memory"),
				translate("Recent items kept in memory per agent"),
				translate("History kept per agent"),
			],
		},
		{ id: "terminals", terms: [translate("Terminals"), translate("Shell integration")] },
		{
			id: "setup-tokens",
			terms: [translate("Claude setup-tokens"), translate("Take setup-tokens")],
		},
	];
	if (record?.os === "macos") {
		cards.push(
			{ id: "sleep", terms: [translate("Sleep"), translate("Keep this Mac awake")] },
			{ id: "cursor", terms: [translate("Cursor usage"), translate("Enable Cursor usage")] }
		);
	}
	if (record?.workspace_host) {
		cards.push({
			id: "workspaces",
			terms: [translate("New workspaces"), translate("Network"), translate("Storage pool")],
		});
	}
	if (offersSkills(view, device)) {
		cards.push({
			id: "library",
			terms: [translate("Skill library"), translate("Holds the skill library")],
		});
	}
	if (view.tables.network_access.get(device)) {
		cards.push({
			id: "network",
			terms: [
				translate("Network access"),
				translate("Port"),
				translate("Addresses for phones"),
			],
		});
	}
	const here = isThisMac(view, device);
	if (here || listsAppKeys(view, device)) {
		cards.push({
			id: "app-keys",
			terms: [
				translate("App keys"),
				...(here ? [translate("This Mac's app key"), translate("Make a new app key")] : []),
			],
		});
	}
	return cards;
};

export const DeviceSettings = ({
	device,
	name,
	live,
	view,
}: {
	device: DeviceId;
	/** The device's name, as the sidebar shows it. */
	name: string;
	/** Whether the device's `maxd` answers, so it can take the change. */
	live: boolean;
	view: ViewState;
}) => {
	useI18n();
	const services = useServices();
	const { source } = services;
	const current = deviceSettingsOf(view, device);
	// What you just picked, shown until the view moves on from what it had
	// then.
	const [pending, setPending] = useState<{ fields: Fields; from: Fields } | null>(null);
	const [note, setNote] = useState<{ part: Part; note: Note } | null>(null);
	const shown = pending && same(pending.from, current) ? pending.fields : current;
	const host = view.tables.device.get(device)?.workspace_host ?? null;
	// The Cursor CLI keeps its login in the Keychain, so only a Mac can read it,
	// and only a Mac sleeps under its `maxd`.
	const mac = view.tables.device.get(device)?.os === "macos";
	// A maxd from before network access publishes none.
	const access = view.tables.network_access.get(device);

	const save = async (part: Part, change: Partial<Fields>) => {
		const fields = { ...shown, ...change };
		if (same(fields, shown)) return;
		setPending({ fields, from: current });
		setNote({ part, note: { text: translate("Saving"), error: false } });
		try {
			const failure = failureOf(await source.setDeviceSettings({ device, ...fields }));
			if (failure) {
				setPending(null);
				setNote({ part, note: { text: failure, error: true } });
			} else {
				setNote({ part, note: { text: translate("Saved"), error: false } });
			}
		} catch (error) {
			console.warn(translate("Couldn't save the device settings"), error);
			setPending(null);
			setNote({
				part,
				note: { text: translate("Couldn't save it. Try again."), error: true },
			});
		}
	};
	const holder = skillLibrary(view)?.device ?? null;
	const moveLibrary = async () => {
		setNote({ part: "library", note: { text: translate("Moving"), error: false } });
		try {
			const failure = failureOf(await source.moveSkillLibrary(device));
			setNote({
				part: "library",
				note: failure
					? { text: failure, error: true }
					: { text: translate("Moved"), error: false },
			});
		} catch (error) {
			console.warn(translate("Couldn't move the skill library"), error);
			setNote({
				part: "library",
				note: { text: translate("Couldn't move it. Try again."), error: true },
			});
		}
	};
	const noteFor = (part: Part) => (note?.part === part ? note.note : null);
	const reject = (part: Part, reason: string) =>
		setNote({ part, note: { text: reason, error: true } });
	const why = settingsWhy(view, device, name, live);
	// Turning setup-tokens on says what that means first.
	const takeTokens = async (on: boolean) => {
		if (on && !(await requestConfirm(services, setupTokenQuestion(name)))) return;
		await save("tokens", { claude_setup_token: on });
	};

	// The card Settings search picked scrolls into view once it shows.
	const reveal = useSettings((s) => s.reveal);
	useEffect(() => {
		if (reveal === null) return;
		document.getElementById(cardId(reveal))?.scrollIntoView?.({ block: "start" });
		revealed(services);
	}, [reveal, services]);

	return (
		<>
			<Section
				title={translate("New agents")}
				id={cardId("new-agents")}
				label={translate("Settings for {{name}}", { name: name })}
				hint={translate("Auto on the Mac and Full access on remote devices by default")}
			>
				<Row
					title={translate("Default mode")}
					sub={
						why.offline ??
						translate(
							"New agents here start in this mode unless you pick another. Running agents keep theirs."
						)
					}
				>
					<StatusNote note={noteFor("mode")} />
					<Picker
						label={translate("Mode")}
						value={shown.default_permission}
						options={modeOptions(undefined)}
						onChange={(picked) => {
							const mode = picked as PermissionMode;
							pickMode(
								services,
								mode,
								() => void save("mode", { default_permission: mode })
							);
						}}
						disabled={!live}
					/>
				</Row>
				<EffortDefaultRows
					view={view}
					device={device}
					name={name}
					live={live}
					shown={shown}
					note={noteFor("effort")}
					onSave={(change) => void save("effort", change)}
				/>
			</Section>
			<Section
				title={translate("Inbox")}
				id={cardId("inbox")}
				label={translate("Inbox settings for {{name}}", { name: name })}
				hint={translate("Idle agents settle after 5 days by default")}
			>
				<Row
					title={translate("Auto-settle after")}
					sub={
						why.inbox ??
						translate(
							"Agents here with no activity for this many days settle on their own, and any new activity brings them back. 0 never settles them."
						)
					}
				>
					<StatusNote note={noteFor("inbox")} />
					<NumberField
						label={translate("Auto-settle after")}
						value={shown.auto_settle_days}
						unit="days"
						disabled={why.inbox !== null}
						onCommit={(text) => {
							const parsed = parseSettleDays(text);
							if (parsed.ok) void save("inbox", { auto_settle_days: parsed.days });
							else reject("inbox", parsed.reason);
						}}
					/>
				</Row>
				<Row
					title={translate("Snooze at the limit")}
					sub={
						why.inbox ??
						translate(
							"Agents that stop at a usage limit snooze until the limit resets, then come back."
						)
					}
				>
					<Switch
						aria-label={translate("Snooze at the limit")}
						checked={shown.snooze_at_limit}
						disabled={why.inbox !== null}
						onCheckedChange={(on) => void save("inbox", { snooze_at_limit: on })}
					/>
				</Row>
			</Section>
			<Section
				title={translate("Usage limits")}
				id={cardId("limits")}
				label={translate("Usage limits on {{name}}", { name: name })}
				hint={translate("Off by default")}
			>
				<Row
					title={translate("Claude Code agents continue")}
					sub={
						why.limits ??
						translate(
							"An agent that a usage limit stops mid-turn gets a message to carry on once the limit resets. This is the default; turn it on or off for one agent from its menu in the sidebar."
						)
					}
				>
					<StatusNote note={noteFor("limits")} />
					<Switch
						aria-label={translate("Claude Code agents continue after the limit")}
						checked={shown.continue_claude_at_limit}
						disabled={why.limits !== null}
						onCheckedChange={(on) =>
							void save("limits", { continue_claude_at_limit: on })
						}
					/>
				</Row>
				<Row
					title={translate("Codex agents continue")}
					sub={
						why.limits ??
						translate(
							"The same for Codex. Cursor agents wait for you, since Cursor doesn't say when a limit resets."
						)
					}
				>
					<Switch
						aria-label={translate("Codex agents continue after the limit")}
						checked={shown.continue_codex_at_limit}
						disabled={why.limits !== null}
						onCheckedChange={(on) =>
							void save("limits", { continue_codex_at_limit: on })
						}
					/>
				</Row>
			</Section>
			<Section
				title={translate("Placement")}
				id={cardId("placement")}
				label={translate("Placement for {{name}}", { name: name })}
				hint={translate("Normal by default")}
			>
				<Row
					title={translate("New agents you don't place")}
					sub={
						why.offline ??
						translate(
							"When more than one device could run a new agent, MaxHQ picks the one with the most free CPU and memory. Prefer makes {{name}} count twice.",
							{ name: name }
						)
					}
				>
					<StatusNote note={noteFor("placement")} />
					<Picker
						label={translate("Placement")}
						value={shown.placement}
						options={PLACEMENT_OPTIONS}
						onChange={(placement) =>
							void save("placement", { placement: placement as Placement })
						}
						disabled={!live}
					/>
				</Row>
			</Section>
			<Section
				title={translate("Idle agents")}
				id={cardId("idle")}
				label={translate("Idle agents on {{name}}", { name: name })}
				hint={translate("1 hour by default")}
			>
				<Row
					title={translate("Stop idle CLIs after")}
					sub={
						why.idle ??
						translate(
							"maxd stops the CLI of an agent idle this long to free its memory. The agent stays as it is and starts again with your next message."
						)
					}
				>
					<StatusNote note={noteFor("idle")} />
					<Picker
						label={translate("Stop idle CLIs after")}
						value={String(shown.idle_release_minutes)}
						options={idleReleaseOptions(shown.idle_release_minutes)}
						onChange={(minutes) =>
							void save("idle", { idle_release_minutes: Number(minutes) })
						}
						disabled={why.idle !== null}
					/>
				</Row>
			</Section>
			<Section
				title={translate("Memory")}
				id={cardId("memory")}
				label={translate("Memory on {{name}}", { name: name })}
				hint={translate("256 items and 256 MB per agent by default")}
			>
				<Row
					title={translate("Recent items kept in memory per agent")}
					sub={
						why.memory ??
						translate(
							"maxd keeps this many of each agent's newest items in memory, and the rest on disk until a client scrolls back. More uses more memory per agent."
						)
					}
				>
					<StatusNote note={noteFor("memory")} />
					<Picker
						label={translate("Recent items kept in memory per agent")}
						value={String(shown.memory_items)}
						options={memoryItemsOptions(shown.memory_items)}
						onChange={(items) => void save("memory", { memory_items: Number(items) })}
						disabled={why.memory !== null}
					/>
				</Row>
				<Row
					title={translate("History kept per agent")}
					sub={
						why.memory ?? (
							<>
								{translate(
									"Past this, an agent's oldest items leave its timeline on this device. They stay in the agent's own history."
								)}
								{shown.history_mib === 0 ? (
									<span className="block text-status-busy-text">
										{NO_LIMIT_WARNING}
									</span>
								) : null}
							</>
						)
					}
				>
					<StatusNote note={noteFor("history")} />
					<Picker
						label={translate("History kept per agent")}
						value={String(shown.history_mib)}
						options={historyOptions(shown.history_mib)}
						wrapDetails
						onChange={(mib) => void save("history", { history_mib: Number(mib) })}
						disabled={why.memory !== null}
					/>
				</Row>
			</Section>
			<Section
				title={translate("Terminals")}
				id={cardId("terminals")}
				label={translate("Terminals on {{name}}", { name: name })}
				hint={translate("Shell integration on by default")}
			>
				<Row
					title={translate("Shell integration")}
					sub={
						why.shell ??
						translate(
							"MaxHQ adds hooks to zsh, bash and fish so it can tell commands apart. Off starts shells exactly as your startup files say. Applies to terminals opened after the change."
						)
					}
				>
					<StatusNote note={noteFor("shell")} />
					<Switch
						aria-label={translate("Shell integration")}
						checked={knowsShellIntegration(view, device) && shown.shell_integration}
						disabled={why.shell !== null}
						onCheckedChange={(on) => void save("shell", { shell_integration: on })}
					/>
				</Row>
			</Section>
			<Section
				title={translate("Claude setup-tokens")}
				id={cardId("setup-tokens")}
				label={translate("Claude setup-tokens for {{name}}", { name: name })}
				hint={translate("Off by default. For personal use")}
			>
				<Row
					title={translate("Take setup-tokens")}
					sub={
						why.tokens ??
						translate(
							"Stores the Claude setup-token you send here and gives it to Claude Code agents when they start. Without it, sign in with claude on this device."
						)
					}
				>
					<StatusNote note={noteFor("tokens")} />
					<Switch
						aria-label={translate("Take setup-tokens")}
						checked={knowsSetupTokens(view, device) ? shown.claude_setup_token : true}
						disabled={why.tokens !== null}
						onCheckedChange={(on) => void takeTokens(on)}
					/>
				</Row>
			</Section>
			{mac ? (
				<>
					<Section
						title={translate("Sleep")}
						id={cardId("sleep")}
						label={translate("Sleep settings for {{name}}", { name: name })}
						hint={translate("On the charger by default")}
					>
						<Row
							title={translate("Keep this Mac awake")}
							sub={
								why.offline ??
								translate(
									"A sleeping Mac stops maxd, so your phone, webhooks and scheduled tasks can't reach it. The display still turns off. Closing the lid still sleeps the Mac."
								)
							}
						>
							<StatusNote note={noteFor("awake")} />
							<Picker
								label={translate("Keep awake")}
								value={shown.keep_awake}
								options={KEEP_AWAKE_OPTIONS}
								wrapDetails
								onChange={(mode) =>
									void save("awake", { keep_awake: mode as KeepAwake })
								}
								disabled={!live}
							/>
						</Row>
					</Section>
					<Section
						title={translate("Cursor usage")}
						id={cardId("cursor")}
						label={translate("Cursor usage for {{name}}", { name: name })}
						hint={translate("Off by default. One Mac covers the whole account")}
					>
						<Row
							title={translate("Enable Cursor usage")}
							sub={
								why.offline ??
								translate(
									"maxd reads the Cursor CLI's saved login from the Keychain, and macOS asks you once to allow it. It then asks Cursor for this month's usage through an endpoint Cursor doesn't document, so it may stop working."
								)
							}
						>
							<StatusNote note={noteFor("cursor")} />
							<Switch
								aria-label={translate("Enable Cursor usage")}
								checked={shown.cursor_usage}
								disabled={!live}
								onCheckedChange={(on) => void save("cursor", { cursor_usage: on })}
							/>
						</Row>
					</Section>
				</>
			) : null}
			{host ? (
				<Section
					title={translate("New workspaces")}
					id={cardId("workspaces")}
					label={translate("Workspace settings for {{name}}", { name: name })}
					hint={translate("What Incus gives each container MaxHQ makes here")}
				>
					<Row
						title={translate("Network")}
						sub={
							why.offline ??
							translate(
								"The Incus network new workspaces join. Pick one with a guard that keeps containers off your network."
							)
						}
					>
						<Picker
							label={translate("Network")}
							value={shown.workspace_network ?? ""}
							options={nameOptions(host.networks)}
							onChange={(network) =>
								void save("workspace", { workspace_network: network })
							}
							disabled={!live}
						/>
					</Row>
					<Row
						title={translate("Storage pool")}
						sub={
							why.offline ??
							translate("The Incus storage pool new workspaces live on.")
						}
					>
						<Picker
							label={translate("Storage pool")}
							value={shown.workspace_pool ?? ""}
							options={nameOptions(host.pools)}
							onChange={(pool) => void save("workspace", { workspace_pool: pool })}
							disabled={!live}
						/>
					</Row>
					{noteFor("workspace") ? (
						<div className="flex justify-end border-t border-sidebar-border px-4 py-2">
							<StatusNote note={noteFor("workspace")} />
						</div>
					) : null}
				</Section>
			) : null}
			{offersSkills(view, device) ? (
				<Section
					title={translate("Skill library")}
					id={cardId("library")}
					label={translate("Skill library for {{name}}", { name: name })}
					hint={translate("One device holds it at a time")}
				>
					<Row
						title={translate("Holds the skill library")}
						sub={
							holder === device
								? translate(
										"{{name}} holds the skill library. To move it, turn this on for another device.",
										{ name: name }
									)
								: why.offline
									? why.offline
									: holder
										? translate(
												"{{deviceName}} holds it now. Turning this on moves it and its sources to {{name}}.",
												{ deviceName: deviceName(view, holder), name: name }
											)
										: translate(
												"No device holds it yet. Turning this on makes {{name}} hold it.",
												{ name: name }
											)
						}
					>
						<StatusNote note={noteFor("library")} />
						<Switch
							aria-label={translate("Holds the skill library")}
							checked={holder === device}
							disabled={!live || holder === device}
							onCheckedChange={(on) => {
								if (on) void moveLibrary();
							}}
						/>
					</Row>
				</Section>
			) : null}
			{access ? (
				<Section
					title={translate("Network access")}
					id={cardId("network")}
					label={translate("Network access for {{name}}", { name: name })}
					hint={translate("Off by default")}
				>
					<Row
						title={translate("Network access")}
						sub={
							<>
								<span className="block">
									{why.offline ??
										translate(
											"Phones on the same network or tailnet can reach {{name}} while it's on.",
											{ name: name }
										)}
								</span>
								{access.problem ? (
									<span className="block text-status-busy-text">
										{access.problem}
									</span>
								) : null}
							</>
						}
					>
						<StatusNote note={noteFor("network")} />
						<Switch
							aria-label={translate("Network access")}
							checked={shown.network.on ?? false}
							disabled={!live}
							onCheckedChange={(on) =>
								void save("network", { network: { ...shown.network, on } })
							}
						/>
					</Row>
					<Row
						title={translate("Port")}
						sub={
							why.offline ??
							translate(
								"The port phones reach {{name}} on. {{DEFAULT_PORT}} by default.",
								{ name: name, DEFAULT_PORT: DEFAULT_PORT }
							)
						}
					>
						<NumberField
							label={translate("Port")}
							value={shown.network.port ?? DEFAULT_PORT}
							disabled={!live}
							onCommit={(text) => {
								const parsed = parsePort(text);
								if (parsed.ok) {
									void save("network", {
										network: { ...shown.network, port: parsed.port },
									});
								} else reject("network", parsed.reason);
							}}
						/>
					</Row>
					{access.on && access.hosts.length > 0 ? (
						<Row
							title={translate("Addresses for phones")}
							sub={access.hosts.join(", ")}
						/>
					) : null}
					{signedIn(view, device).map((signed) => (
						<PhoneSessionRow key={signed.session.id} signed={signed} />
					))}
				</Section>
			) : null}
			<AcpAgents device={device} name={name} live={live} view={view} />
			<AppKeys id={cardId("app-keys")} device={device} name={name} live={live} view={view} />
			<DeviceLinks device={device} name={name} />
			<PhoneAddress device={device} name={name} />
		</>
	);
};
