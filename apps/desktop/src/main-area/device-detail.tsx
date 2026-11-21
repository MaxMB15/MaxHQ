// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One device in the main area: how it's reached, the chain of checks from
// this Mac to its agents with the one that stopped marked, its load, CLIs,
// routes and settings, and the agents it runs or last ran, with the
// settled ones last. This Mac's page also starts and uninstalls its maxd.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { CircleCheck } from "lucide-react";
import { type CSSProperties, useEffect, useRef, useState } from "react";

import { openAgent } from "@/app/open-agent";
import { useServices } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import { ActivityDot, ConnectionDot } from "@/components/status/dots";
import type { Agent, DeviceId, DeviceLink, LinkState, Timestamp } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { cn } from "@/lib/utils";
import { Picker } from "@/new-agent/picker";
import { PRIMARY, Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { DeviceSettings } from "@/settings/device-settings";
import { failureOf, type Note } from "@/settings/result";
import { ACTIVITY_LABEL, agentLook } from "@maxhq/shared/status/activity";
import type { Check, CheckState } from "@maxhq/shared/status/card";
import { deviceConnection, linkChecks } from "@maxhq/shared/status/connection";
import {
	DEFAULT_WIND_DOWN_MINUTES,
	deviceChips,
	splitDetail,
	WIND_DOWN_MINUTES,
} from "@maxhq/shared/status/device-page";
import { formatDuration, formatWhen } from "@maxhq/shared/status/format";
import { versionNote } from "@maxhq/shared/status/reach";
import { kindName, ownKind } from "@maxhq/shared/status/agent-kind";
import { accountName } from "@maxhq/shared/status/usage";
import { inboxOf } from "@maxhq/shared/view/inbox";
import type { ViewState } from "@maxhq/shared/view/replica";
import type { DeviceNode } from "@maxhq/shared/view/select";
import { DeviceTestHosts } from "@/test-hosts/device-test-hosts";
import { DeviceMoves } from "@/workspaces/device-moves";
import { DeviceWorkspaces } from "@/workspaces/workspace-list";

import { AdmissionCards } from "./admission-cards";
import { DeviceIdentity, LinkClaimCard } from "./device-identity";
import { DeviceInstalls } from "./device-installs";
import { DeviceLoad } from "./device-load";
import { DeviceRegistry } from "./device-registry";
import { DeviceRoutes } from "./device-routes";
import { DeviceSetup } from "./device-setup";
import { DeviceTools } from "./device-tools";
import { LocalMaxd, StartMaxdButton, useStartMaxd } from "./local-maxd";

const CheckIcon = ({ state }: { state: CheckState }) => {
	const { t } = useI18n();

	switch (state) {
		case "passed":
		case "running":
			return (
				<span
					role="img"
					aria-label={state === "passed" ? t("Passed") : t("Running")}
					className={cn(
						"grid size-3.5 place-items-center rounded-full",
						state === "passed" ? "bg-status-ready/18" : "bg-status-connecting/20"
					)}
				>
					{state === "passed" ? (
						<svg
							aria-hidden
							width={9}
							height={9}
							viewBox="0 0 24 24"
							fill="none"
							stroke="var(--status-ready)"
							strokeWidth={3.6}
							strokeLinecap="round"
							strokeLinejoin="round"
						>
							<path d="M5 12l5 5 9-10" />
						</svg>
					) : (
						<span className="size-2.5 animate-busy rounded-full border-[1.5px] border-status-connecting/28 border-t-status-connecting" />
					)}
				</span>
			);
		case "failed":
			return (
				<span
					role="img"
					aria-label={t("Failed")}
					className="grid size-3.5 place-items-center rounded-full bg-status-error/20"
				>
					<svg
						aria-hidden
						width={8}
						height={8}
						viewBox="0 0 24 24"
						fill="none"
						stroke="var(--status-error)"
						strokeWidth={4}
						strokeLinecap="round"
					>
						<path d="M6 6l12 12M18 6L6 18" />
					</svg>
				</span>
			);
		case "not_run":
			return (
				<span
					role="img"
					aria-label={t("Not run")}
					className="block size-3.5 rounded-full border-[1.5px] border-dashed border-status-off"
				/>
			);
		case "unknown":
			return (
				<span
					role="img"
					aria-label={t("Unknown")}
					className="grid size-3.5 place-items-center rounded-full border-[1.5px] border-status-off text-[9px] leading-none font-bold text-muted-foreground"
				>
					?
				</span>
			);
	}
};

const CheckRow = ({ check, first }: { check: Check; first: boolean }) => {
	useI18n();

	const failed = check.state === "failed";
	const [main, sub] = failed ? splitDetail(check.detail) : [check.detail, undefined];
	return (
		<div
			role="listitem"
			className={cn(
				"grid grid-cols-[38px_180px_minmax(0,1fr)] items-center px-4",
				!first && "border-t border-row-divider",
				failed ? "min-h-[52px] bg-status-error/6" : "h-10"
			)}
		>
			<span className="flex items-center">
				<CheckIcon state={check.state} />
			</span>
			<span
				className={cn(
					"truncate text-[12.5px]",
					failed
						? "font-semibold text-foreground-strong"
						: check.state === "not_run"
							? "text-muted-foreground"
							: "text-foreground"
				)}
			>
				{check.title}
			</span>
			<span className={cn("min-w-0 text-xs", failed ? "py-2" : "truncate")}>
				<span
					className={cn(
						"selectable",
						failed ? "text-status-error-text" : "text-muted-foreground"
					)}
				>
					{main}
				</span>
				{sub ? (
					<span className="selectable block text-[11.5px] text-muted-foreground">
						{sub}
					</span>
				) : null}
			</span>
		</div>
	);
};

const button =
	"inline-flex h-[30px] shrink-0 items-center rounded-[7px] px-3 text-[12.5px] disabled:opacity-60";

// Start maxd in the header, while this Mac's maxd isn't connected.
const StartHere = () => {
	const { busy, note, start } = useStartMaxd();
	return (
		<>
			<StatusNote note={note} className="max-w-[24ch] self-center" />
			<StartMaxdButton busy={busy} start={start} />
		</>
	);
};

const Actions = ({ node }: { node: DeviceNode }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const route = node.link.route;
	const start = route.type === "local" && node.link.state.state !== "connected";
	return (
		<div className="flex shrink-0 gap-2">
			{start ? <StartHere /> : null}
			<button
				type="button"
				onClick={() => void source.refreshLinks(node.link.id)}
				className={start ? SECONDARY : PRIMARY}
			>
				{t("Try now")}
			</button>
			{route.type === "ssh" ? (
				<CopyButton
					text={`ssh ${route.host}`}
					label={t("Copy SSH command")}
					icon={false}
					className={cn(
						button,
						"gap-1.5 border border-input bg-secondary text-foreground hover:bg-accent"
					)}
				/>
			) : null}
		</div>
	);
};

const AgentLine = ({
	agent,
	view,
	now,
	live,
}: {
	agent: Agent;
	view: ViewState;
	now: Timestamp;
	live: boolean;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const workspace = view.tables.workspace.get(agent.workspace);
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const where = [
		workspace?.location.type === "container" ? workspace.location.instance : undefined,
		account ? accountName(account) : kindName(ownKind(agent)),
	]
		.filter(Boolean)
		.join(", ");
	const look = agentLook(agent, now);
	const settled = inboxOf(agent).settled;
	const state = settled
		? t("Settled {{value1}}", { value1: formatWhen(settled.at, now) })
		: live
			? ACTIVITY_LABEL[look]
			: t("Last seen {{value1}}", { value1: formatWhen(agent.last_activity_at, now) });
	return (
		<button
			type="button"
			onClick={() => openAgent(services, agent.id)}
			className="flex h-[34px] w-full items-center gap-2.5 rounded-lg border border-sidebar-border bg-card px-3 text-left hover:bg-secondary"
		>
			{settled ? (
				<CircleCheck
					aria-hidden
					size={12}
					strokeWidth={2}
					className="shrink-0 text-muted-foreground"
				/>
			) : (
				<ActivityDot look={look} />
			)}
			<span className="min-w-0 flex-1 truncate text-[12.5px] text-terminal">
				{agent.title}
			</span>
			<span className="shrink-0 truncate text-xs text-muted-foreground">{where}</span>
			<span className="w-[136px] shrink-0 truncate text-right text-xs text-muted-foreground">
				{state}
			</span>
		</button>
	);
};

/** The banner while a device winds down for a restart, and after. */
export const WindingDown = ({
	name,
	until,
	now,
}: {
	name: string;
	until: Timestamp;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const ahead = until > now;
	return (
		<div
			role="status"
			aria-label={t("Winding down")}
			className="flex items-start gap-3 rounded-[11px] border border-status-connecting/40 bg-status-connecting/10 px-4 py-3"
		>
			<ConnectionDot look="connecting" size={10} decorative className="mt-[3px]" />
			<div className="flex min-w-0 flex-col gap-0.5">
				<span className="text-[13px] font-[650] text-status-connecting-text">
					{ahead
						? t("{{name}} is winding down for a restart", { name: name })
						: t("{{name}} has wound down for a restart", { name: name })}
				</span>
				<span className="text-xs text-secondary-foreground">
					{ahead
						? t(
								"Busy agents are finishing the step they're on. The window closes at {{value1}}, in {{value2}}.",
								{
									value1: formatWhen(until, now),
									value2: formatDuration(until - now),
								}
							)
						: t(
								"The window closed at {{value1}}. If {{name}} drops for its restart, MaxHQ tries it every 2 seconds for 10 minutes, and its agents resume once it's back.",
								{ value1: formatWhen(until, now), name: name }
							)}
				</span>
			</div>
		</div>
	);
};

const WindDown = ({
	device,
	name,
	live,
	winding,
}: {
	device: DeviceId;
	name: string;
	live: boolean;
	/** Whether a window is already open. */
	winding: boolean;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [minutes, setMinutes] = useState(DEFAULT_WIND_DOWN_MINUTES);
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const start = async () => {
		setBusy(true);
		setNote({ text: t("Sending"), error: false });
		try {
			const failure = failureOf(await source.windDown(device, minutes * 60));
			setNote(failure ? { text: failure, error: true } : null);
		} catch (error) {
			console.warn("Couldn't wind the device down", error);
			setNote({ text: t("Couldn't reach it. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};
	return (
		<Row
			title={t("Wind down first")}
			sub={
				live
					? t(
							"Asks the busy agents MaxHQ runs on {{name}} to finish the step they're on and start no long command. Restart {{name2}} once the window closes, and they pick up after it.",
							{ name: name, name2: name }
						)
					: t("Connect to {{name}} to wind it down.", { name: name })
			}
		>
			<StatusNote note={note} />
			<Picker
				label={t("Window")}
				value={String(minutes)}
				options={WIND_DOWN_MINUTES.map((m) => ({
					value: String(m),
					label: t("{{m}} min", { m: formatNumber(m) }),
				}))}
				onChange={(value) => setMinutes(Number(value))}
				disabled={!live || busy}
			/>
			<button
				type="button"
				className={SECONDARY}
				disabled={!live || busy || winding}
				onClick={() => void start()}
			>
				{t("Wind down for restart")}
			</button>
		</Row>
	);
};

// When the link last connected, or null while it isn't connected.
const connectedSince = (state: LinkState): Timestamp | null =>
	state.state === "connected" ? state.since : null;

/**
 * Restarts the device's maxd through its service manager. It asks nothing
 * of maxd, so it stays on while the device isn't live, even for a maxd too
 * old for the app. "Restarting" stays until the link connects again.
 */
const RestartMaxd = ({ link }: { link: DeviceLink }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	// The connection the restart started from, and whether it has dropped
	// since, while a restart waits for the link to come back.
	const waiting = useRef<{ since: Timestamp | null; dropped: boolean } | null>(null);
	const since = connectedSince(link.state);
	useEffect(() => {
		const wait = waiting.current;
		if (!wait) return;
		if (since === null) {
			wait.dropped = true;
		} else if (wait.dropped || since !== wait.since) {
			waiting.current = null;
			setNote(null);
		}
	}, [since]);
	const restart = async () => {
		setBusy(true);
		setNote({ text: t("Restarting"), error: false });
		waiting.current = { since, dropped: false };
		try {
			const failure = failureOf(await source.restartMaxd(link.id));
			if (failure) {
				waiting.current = null;
				setNote({ text: failure, error: true });
			}
		} catch (error) {
			console.warn("Couldn't restart maxd", error);
			waiting.current = null;
			setNote({ text: t("Couldn't restart maxd. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};
	return (
		<Row
			title={t("Restart maxd")}
			sub={t(
				"Stops maxd on {{name}} and starts it again. Agents keep running in their own processes and pick up once it's back.",
				{ name: link.name }
			)}
		>
			<StatusNote note={note} />
			<button
				type="button"
				className={SECONDARY}
				disabled={busy}
				onClick={() => void restart()}
			>
				{t("Restart maxd")}
			</button>
		</Row>
	);
};

const Restart = ({
	link,
	live,
	winding,
}: {
	link: DeviceLink;
	live: boolean;
	winding: boolean;
}) => {
	const { t } = useI18n();
	return (
		<Section
			title={t("Restart")}
			label={t("Restart {{name}}", { name: link.name })}
			hint={t("For a restart you plan, such as an update")}
		>
			{link.device ? (
				<WindDown device={link.device} name={link.name} live={live} winding={winding} />
			) : null}
			<RestartMaxd link={link} />
		</Section>
	);
};

/** The main area for one device. */
export const DeviceDetail = ({ node, view }: { node: DeviceNode; view: ViewState }) => {
	const { t } = useI18n();

	const now = useNow(1000);
	const { link } = node;
	const connection = deviceConnection(node.ctx, now);
	const checks = linkChecks(node.ctx, now);
	const live = connection.look === "live";
	const until = node.ctx.device?.winding_down_until ?? null;
	const version = versionNote(link);
	const tone =
		connection.look === "unreachable"
			? "text-status-error-text"
			: connection.look === "connecting"
				? "text-status-connecting-text"
				: connection.look === "agent_down"
					? "text-status-linked"
					: "text-secondary-foreground";

	return (
		<div
			className="h-full overflow-y-auto bg-background px-8 py-7"
			style={{ "--dot-surface": "var(--background)" } as CSSProperties}
		>
			<div className="flex max-w-[760px] flex-col gap-[22px]">
				<div className="flex items-start gap-4">
					<div className="flex min-w-0 flex-1 flex-col gap-2">
						<h1 className="truncate text-[22px] font-[650] tracking-[-0.015em] text-foreground-strong selectable">
							{link.name}
						</h1>
						<p className={cn("text-[13px]", tone)}>
							<span className="font-semibold">{connection.card.title}.</span>{" "}
							<span className="selectable">{connection.card.body}</span>
						</p>
						<div className="flex flex-wrap gap-1.5">
							{deviceChips(node).map((chip) => (
								<span
									key={chip}
									className="selectable inline-flex h-5 items-center rounded-[5px] border border-track bg-secondary px-[7px] text-[11.5px] text-secondary-foreground"
								>
									{chip}
								</span>
							))}
						</div>
						{version ? (
							<p className="selectable text-[12px] text-muted-foreground">
								{version}
							</p>
						) : null}
					</div>
					<Actions node={node} />
				</div>

				{until !== null ? <WindingDown name={link.name} until={until} now={now} /> : null}

				<LinkClaimCard link={link} view={view} now={now} />
				{link.device ? <AdmissionCards device={link.device} view={view} /> : null}

				<section
					aria-label={t("Connection checks")}
					className="overflow-hidden rounded-[11px] border border-track bg-card"
				>
					<div className="flex items-baseline justify-between gap-3 border-b border-sidebar-border px-4 py-3">
						<h2 className="text-[13.5px] font-[650] text-foreground">
							{t("Where the connection stops")}
						</h2>
						<span className="text-[11.5px] text-muted-foreground">
							{t("Each check runs only if the one above passed")}
						</span>
					</div>
					<div role="list" className="flex flex-col">
						{checks.map((check, i) => (
							<CheckRow key={check.id} check={check} first={i === 0} />
						))}
					</div>
					{!live ? (
						<div className="flex items-center gap-2 border-t border-sidebar-border bg-subpanel px-4 py-2.5 text-xs text-muted-foreground">
							<svg
								aria-hidden
								width={13}
								height={13}
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								strokeWidth={2}
								strokeLinecap="round"
							>
								<circle cx="12" cy="12" r="9" />
								<path d="M12 7v5l3 2" />
							</svg>
							<span>{t("Try now runs every check again from the top.")}</span>
						</div>
					) : null}
				</section>

				{link.device ? (
					<>
						<DeviceLoad device={link.device} name={link.name} live={live} view={view} />
						<DeviceSetup
							device={link.device}
							name={link.name}
							live={live}
							view={view}
						/>
						<DeviceWorkspaces device={link.device} />
						<DeviceMoves
							device={link.device}
							name={link.name}
							live={live}
							view={view}
						/>
						<DeviceTools
							device={link.device}
							name={link.name}
							live={live}
							view={view}
						/>
						<DeviceInstalls
							device={link.device}
							name={link.name}
							live={live}
							view={view}
						/>
						<DeviceRegistry
							device={link.device}
							name={link.name}
							live={live}
							view={view}
						/>
						<DeviceTestHosts
							device={link.device}
							name={link.name}
							live={live}
							view={view}
						/>
						<DeviceRoutes link={link.id} view={view} />
						<DeviceSettings
							device={link.device}
							name={link.name}
							live={live}
							view={view}
						/>
					</>
				) : null}
				<DeviceIdentity link={link} view={view} />

				<Restart link={link} live={live} winding={until !== null && until > now} />
				{link.route.type === "local" ? <LocalMaxd link={link} /> : null}

				<section
					aria-label={t("Agents on {{name}}", { name: link.name })}
					className="flex flex-col gap-1.5"
				>
					<h2 className="text-[13.5px] font-[650] text-foreground">
						{live ? t("Agents here") : t("Agents last seen here")}
					</h2>
					{node.agents.map((agent) => (
						<AgentLine key={agent.id} agent={agent} view={view} now={now} live={live} />
					))}
					{node.agents.length === 0 ? (
						<p className="text-xs text-muted-foreground">
							{node.settled.length > 0
								? t("Every agent on {{name}} is settled.", { name: link.name })
								: t("No agents on {{name}}.", { name: link.name })}
						</p>
					) : null}
					{!live && node.agents.length > 0 ? (
						<p className="max-w-[64ch] text-xs text-muted-foreground">
							{t(
								"These are the agents MaxHQ saw before it lost touch with {{name}}. Their state updates once it answers again.",
								{ name: link.name }
							)}
						</p>
					) : null}
					{node.settled.length > 0 ? (
						<>
							<h3 className="mt-2 text-xs font-[650] text-muted-foreground">
								{t("Settled")}
							</h3>
							{node.settled.map((agent) => (
								<AgentLine
									key={agent.id}
									agent={agent}
									view={view}
									now={now}
									live={live}
								/>
							))}
						</>
					) : null}
				</section>
			</div>
		</div>
	);
};
