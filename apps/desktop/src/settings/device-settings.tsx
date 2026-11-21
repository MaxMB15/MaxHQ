// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A device's tab in Settings: how it's reached, the mode new agents on it
// start in with their usage limits and placement, when its idle agents
// settle, the resources its agents get (how long an idle agent keeps its
// CLI running, how much of each agent's timeline its `maxd` keeps, and on
// a Mac when it keeps the Mac awake), whether its terminals mark commands,
// whether it takes Claude setup-tokens, and, on a device with Incus, the
// network and storage pool new workspaces get, whether it holds the skill
// library, whether phones reach it over the network, and its test
// machines. Its `maxd` keeps them and reports them, and a save sends them
// all, so changing one keeps the others as the device has them. Holding
// the skill library is a move of its own, since only one device holds it.
// The phones signed in over the network each have a Revoke.
//
// Two settings every device has show on the page instead, a row for each
// device: whether its agents read other agents, in Permissions, and on a
// Mac whether it reads Cursor usage, in Accounts. Settings search finds
// the cards by their headings and labels.

import { type KeyboardEvent, useState } from "react";

import { type Services, useServices, useViewOf } from "@/app/services";
import { requestConfirm } from "@/components/confirm";
import { Switch } from "@/components/ui/switch";
import type { DeviceId, PermissionMode } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import type { KeepAwake, Placement } from "@maxhq/shared/fleet/wire";
import type { DeviceCardId } from "@maxhq/shared/guide/settings-link";
import { Picker } from "@/new-agent/picker";
import { modeOptions } from "@maxhq/shared/new-agent/spec";
import type { EntityKind, ViewState } from "@maxhq/shared/view/replica";
import { cn } from "@/lib/utils";
import { accountDevices } from "@maxhq/shared/settings/accounts";
import { connectionFacts } from "@maxhq/shared/settings/connection";
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
import { knowsReadRights } from "@maxhq/shared/settings/read-rights";
import { knowsSetupTokens, setupTokenQuestion } from "@maxhq/shared/settings/setup-tokens";
import { knowsShellIntegration } from "@maxhq/shared/settings/shell-integration";
import { offersSkills, skillLibrary } from "@maxhq/shared/skills/library";
import { kindLabel, offersTestHosts } from "@maxhq/shared/test-hosts/hosts";
import { deviceLive, placesOn } from "@maxhq/shared/test-hosts/place";
import { deviceName } from "@maxhq/shared/view/select";
import { TestMachines } from "@/test-hosts/device-test-hosts";

import { pickMode } from "./auto-consent";
import {
	CardScope,
	ConfirmButton,
	FIELD,
	Part as CardPart,
	Row,
	Section,
	StatusNote,
} from "./controls";
import { EffortDefaultRows } from "./effort-defaults";
import { PhoneAddress } from "./phones-settings";
import { failureOf, type Note } from "./result";
import { deviceCardId } from "./store";
import { Value } from "./value";
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
	| "reads"
	| "tokens";

/**
 * A number field. What you type stays here until you leave the field or
 * press Enter, so a half-typed number never reaches the device.
 */
export const NumberField = ({
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
		<Row
			title={<span className="selectable">{signed.name}</span>}
			sub={sessionLine(signed.session, now)}
		>
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

/** A card on a device's tab, as Settings search sees it. */
export type DeviceCard = {
	readonly id: DeviceCardId;
	/** Its heading, then its controls' labels. */
	readonly terms: readonly string[];
};

/** The cards {@link DeviceSettings} shows for `device`, in order. */
export const deviceCards = (view: ViewState, device: DeviceId): readonly DeviceCard[] => {
	const record = view.tables.device.get(device);
	const mac = record?.os === "macos";
	const cards: DeviceCard[] = [
		{
			id: "connection",
			terms: [
				translate("Connection"),
				...connectionFacts(view, device).map((fact) => fact.label),
				translate("Address for the phone"),
			],
		},
		{
			id: "new-agents",
			terms: [
				translate("New agents"),
				translate("Default mode"),
				translate("Effort"),
				translate("Usage limits"),
				translate("Claude Code agents continue"),
				translate("Codex agents continue"),
				translate("Continue after the limit"),
				translate("Placement"),
				translate("New agents you don't place"),
			],
		},
		{
			id: "inbox",
			terms: [
				translate("Inbox"),
				translate("Auto-settle after"),
				translate("Snooze at the limit"),
			],
		},
		{
			id: "resources",
			terms: [
				translate("Resources"),
				translate("Idle agents"),
				translate("Stop idle CLIs after"),
				translate("Memory"),
				translate("Recent items kept in memory per agent"),
				translate("History kept per agent"),
				...(mac ? [translate("Sleep"), translate("Keep this Mac awake")] : []),
			],
		},
		{ id: "terminals", terms: [translate("Terminals"), translate("Shell integration")] },
		{
			id: "setup-tokens",
			terms: [translate("Claude setup-tokens"), translate("Take setup-tokens")],
		},
	];
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
	if (offersTestHosts(view, device)) {
		cards.push({
			id: "test-machines",
			terms: [
				translate("Test machines"),
				translate("Test hosts"),
				...placesOn(view, device).map((place) => kindLabel(place.kind)),
			],
		});
	}
	return cards;
};

/**
 * One device's settings as you change them: what to show, which is your
 * pick until the view moves on from what it had then, the save that sends
 * them all, the note beside the part you changed, and why a part can't
 * change now.
 */
const useDeviceFields = (view: ViewState, device: DeviceId, name: string, live: boolean) => {
	const { source } = useServices();
	const current = deviceSettingsOf(view, device);
	const [pending, setPending] = useState<{ fields: Fields; from: Fields } | null>(null);
	const [note, setNote] = useState<{ part: Part; note: Note } | null>(null);
	const shown = pending && same(pending.from, current) ? pending.fields : current;

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
	return {
		shown,
		save,
		setNote: (part: Part, said: Note) => setNote({ part, note: said }),
		noteFor: (part: Part) => (note?.part === part ? note.note : null),
		reject: (part: Part, reason: string) =>
			setNote({ part, note: { text: reason, error: true } }),
		why: settingsWhy(view, device, name, live),
	};
};

/** The Connection card: what the view knows about how the device is reached, each to copy. */
const Connection = ({ device, view }: { device: DeviceId; view: ViewState }) => {
	useI18n();
	return (
		<Section title={translate("Connection")} id={deviceCardId("connection")}>
			<dl className="grid grid-cols-[150px_minmax(0,1fr)] items-center gap-x-3.5 gap-y-2 px-4 pt-2.5 pb-3.5 text-[12.5px]">
				{connectionFacts(view, device).map((fact) => (
					<div key={fact.label} className="contents">
						<dt className="text-muted-foreground">{fact.label}</dt>
						<dd className="flex min-w-0">
							<Value text={fact.value} what={fact.label} />
						</dd>
					</div>
				))}
			</dl>
			<PhoneAddress device={device} />
		</Section>
	);
};

// What the device's tab reads from the view.
const KINDS: readonly EntityKind[] = [
	"device",
	"device_settings",
	"device_tools",
	"device_link",
	"network",
	"account",
	"network_access",
	"phone_session",
	"phone",
	"skill_library",
	"test_host",
	"test_lease",
];

/** The cards of `device`'s tab, or of those in `only`, while a search picks some. */
export const DeviceSettings = ({
	device,
	name,
	only,
}: {
	device: DeviceId;
	/** The device's name, as the sidebar shows it. */
	name: string;
	only?: ReadonlySet<DeviceCardId> | null;
}) => {
	useI18n();
	const view = useViewOf(KINDS);
	const now = useNow(5000);
	const live = deviceLive(view, device, now);
	const listed = new Set(deviceCards(view, device).map((card) => card.id));
	const shows = (card: DeviceCardId) => listed.has(card) && (!only || only.has(card));
	return (
		<CardScope.Provider value={name}>
			{shows("connection") ? <Connection device={device} view={view} /> : null}
			<Cards device={device} name={name} live={live} view={view} shows={shows} />
			{shows("test-machines") ? (
				<TestMachines device={device} name={name} live={live} view={view} />
			) : null}
		</CardScope.Provider>
	);
};

// The cards that save the device's settings, in order.
const Cards = ({
	device,
	name,
	live,
	view,
	shows,
}: {
	device: DeviceId;
	name: string;
	/** Whether the device's `maxd` answers, so it can take the change. */
	live: boolean;
	view: ViewState;
	shows: (card: DeviceCardId) => boolean;
}) => {
	useI18n();
	const services = useServices();
	const { shown, save, setNote, noteFor, reject, why } = useDeviceFields(
		view,
		device,
		name,
		live
	);
	const host = view.tables.device.get(device)?.workspace_host ?? null;
	// Only a Mac sleeps under its `maxd`.
	const mac = view.tables.device.get(device)?.os === "macos";
	// A maxd from before network access publishes none.
	const access = view.tables.network_access.get(device);
	const holder = skillLibrary(view)?.device ?? null;
	const moveLibrary = () =>
		moveSkillLibrary(services, device, (said) => setNote("library", said));
	// Turning setup-tokens on says what that means first.
	const takeTokens = async (on: boolean) => {
		if (on && !(await requestConfirm(services, setupTokenQuestion(name)))) return;
		await save("tokens", { claude_setup_token: on });
	};

	return (
		<>
			{shows("new-agents") ? (
				<Section
					title={translate("New agents")}
					id={deviceCardId("new-agents")}
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
					<CardPart
						title={translate("Usage limits")}
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
								aria-label={translate(
									"Claude Code agents continue after the limit"
								)}
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
					</CardPart>
					<CardPart
						title={translate("Placement")}
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
					</CardPart>
				</Section>
			) : null}
			{shows("inbox") ? (
				<Section
					title={translate("Inbox")}
					id={deviceCardId("inbox")}
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
								if (parsed.ok)
									void save("inbox", { auto_settle_days: parsed.days });
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
			) : null}
			{shows("resources") ? (
				<Section title={translate("Resources")} id={deviceCardId("resources")}>
					<CardPart
						title={translate("Idle agents")}
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
					</CardPart>
					<CardPart
						title={translate("Memory")}
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
								onChange={(items) =>
									void save("memory", { memory_items: Number(items) })
								}
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
								onChange={(mib) =>
									void save("history", { history_mib: Number(mib) })
								}
								disabled={why.memory !== null}
							/>
						</Row>
					</CardPart>
					{mac ? (
						<CardPart
							title={translate("Sleep")}
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
						</CardPart>
					) : null}
				</Section>
			) : null}
			{shows("terminals") ? (
				<Section
					title={translate("Terminals")}
					id={deviceCardId("terminals")}
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
			) : null}
			{shows("setup-tokens") ? (
				<Section
					title={translate("Claude setup-tokens")}
					id={deviceCardId("setup-tokens")}
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
							checked={
								knowsSetupTokens(view, device) ? shown.claude_setup_token : true
							}
							disabled={why.tokens !== null}
							onCheckedChange={(on) => void takeTokens(on)}
						/>
					</Row>
				</Section>
			) : null}
			{host && shows("workspaces") ? (
				<Section
					title={translate("New workspaces")}
					id={deviceCardId("workspaces")}
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
			{shows("library") ? (
				<Section
					title={translate("Skill library")}
					id={deviceCardId("library")}
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
			{access && shows("network") ? (
				<Section
					title={translate("Network access")}
					id={deviceCardId("network")}
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
		</>
	);
};

/** Moves the skill library to `device`, saying how it went. */
const moveSkillLibrary = async (
	services: Services,
	device: DeviceId,
	say: (note: Note) => void
) => {
	say({ text: translate("Moving"), error: false });
	try {
		const failure = failureOf(await services.source.moveSkillLibrary(device));
		say(failure ? { text: failure, error: true } : { text: translate("Moved"), error: false });
	} catch (error) {
		console.warn(translate("Couldn't move the skill library"), error);
		say({ text: translate("Couldn't move it. Try again."), error: true });
	}
};

/** The tables the page's rows for every device read. */
const EVERY_DEVICE: readonly EntityKind[] = ["device", "device_settings", "device_link", "network"];

/** A device's row in the Other agents card. */
const ReadsRow = ({ device, name, view }: { device: DeviceId; name: string; view: ViewState }) => {
	useI18n();
	const now = useNow(5000);
	const live = deviceLive(view, device, now);
	const { shown, save, noteFor, why } = useDeviceFields(view, device, name, live);
	return (
		<Row title={<span className="selectable">{name}</span>} sub={why.reads}>
			<StatusNote note={noteFor("reads")} />
			<Switch
				aria-label={translate("{{value1}} on {{name}}", {
					value1: translate("Agents can read other agents"),
					name,
				})}
				checked={knowsReadRights(view, device) && shown.read_other_agents}
				disabled={why.reads !== null}
				onCheckedChange={(on) => void save("reads", { read_other_agents: on })}
			/>
		</Row>
	);
};

/** The Other agents card's labels, for the settings search. */
export const OTHER_AGENTS_TERMS: readonly string[] = ["Agents can read other agents"];

/** The Other agents card: whether agents on each device read other agents. */
export const OtherAgentsCard = () => {
	useI18n();
	const view = useViewOf(EVERY_DEVICE);
	return (
		<Section title={translate("Other agents")} hint={translate("Reading on by default")}>
			<Row
				title={translate("Agents can read other agents")}
				sub={translate(
					"Any agent can list and read any other agent's conversation, which may hold code, paths and command output. Off lets agents read only the agents you attach or quote. Starting, messaging or stopping agents still needs your permission."
				)}
			/>
			{accountDevices(view).map(({ device, name }) => (
				<ReadsRow key={device} device={device} name={name} view={view} />
			))}
		</Section>
	);
};

/** A Mac's row in the Cursor usage card. */
const CursorRow = ({ device, name, view }: { device: DeviceId; name: string; view: ViewState }) => {
	useI18n();
	const now = useNow(5000);
	const live = deviceLive(view, device, now);
	const { shown, save, noteFor, why } = useDeviceFields(view, device, name, live);
	return (
		<Row title={<span className="selectable">{name}</span>} sub={why.offline}>
			<StatusNote note={noteFor("cursor")} />
			<Switch
				aria-label={translate("{{value1}} on {{name}}", {
					value1: translate("Enable Cursor usage"),
					name,
				})}
				checked={shown.cursor_usage}
				disabled={!live}
				onCheckedChange={(on) => void save("cursor", { cursor_usage: on })}
			/>
		</Row>
	);
};

/** The Cursor usage card's labels, for the settings search. */
export const CURSOR_USAGE_TERMS: readonly string[] = ["Enable Cursor usage"];

/**
 * The Cursor usage card, a row for each Mac, since the Cursor CLI keeps
 * its login in the Keychain and only a Mac can read it. Nothing shows
 * without a Mac.
 */
export const CursorUsageCard = () => {
	useI18n();
	const view = useViewOf(EVERY_DEVICE);
	const macs = accountDevices(view).filter(
		({ device }) => view.tables.device.get(device)?.os === "macos"
	);
	if (macs.length === 0) return null;
	return (
		<Section
			title={translate("Cursor usage")}
			hint={translate("Off by default. One Mac covers the whole account")}
		>
			<Row
				title={translate("Enable Cursor usage")}
				sub={translate(
					"maxd reads the Cursor CLI's saved login from the Keychain, and macOS asks you once to allow it. It then asks Cursor for this month's usage through an endpoint Cursor doesn't document, so it may stop working."
				)}
			/>
			{macs.map(({ device, name }) => (
				<CursorRow key={device} device={device} name={name} view={view} />
			))}
		</Section>
	);
};
