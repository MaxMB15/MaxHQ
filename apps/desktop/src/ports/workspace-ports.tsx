// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where an agent runs, in its header: the device, the container and how
// many ports reach this Mac. It opens a popover over the workspace's ports
// and what the agent runs, per N-6: the forwarded ports with Open and Stop,
// the agents' other ports with Forward, the quiet list of ports no agent
// opened, then the processes the agent started. Ports lead because a busy
// agent can run a long list of short commands. On this Mac
// a port needs no forward, so each one offers Open instead. A device that
// doesn't forward ports lists them with neither, and so does one that can't
// be reached right now.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Box, ExternalLink, Folder, Laptop, Plug, Server, Unplug } from "lucide-react";

import { useView } from "@/app/services";
import { AgentConnectionDot } from "@/components/status/agent-dots";
import { Popover, PopoverPopup, PopoverTitle, PopoverTrigger } from "@/components/ui/popover";
import type { Agent, Forward, ListeningPort, Workspace } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { all } from "@maxhq/shared/view/replica";
import { deviceName, linkForDevice } from "@maxhq/shared/view/select";

import { type PortActions, usePortActions } from "./actions";
import { Failure, ListeningRow, PortNumber, Process, Section } from "./parts";
import { Running } from "./running";
import {
	forwardOf,
	forwardsPorts,
	isRemoteDevice,
	isUnreachable,
	placeName,
	workspacePorts,
} from "@maxhq/shared/ports/select";
import { dismissPort } from "@maxhq/shared/ports/store";

const ICON_BUTTON =
	"grid size-6 cursor-pointer place-items-center rounded-[5px] text-muted-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:opacity-60";

const ForwardedRow = ({
	port,
	forward,
	actions,
}: {
	port: ListeningPort;
	forward: Forward;
	actions: PortActions;
}) => {
	const { t } = useI18n();

	const local = `localhost:${forward.local_port}`;
	return (
		<li className="grid grid-cols-[46px_minmax(0,1fr)_112px_56px] items-center gap-x-2 gap-y-0.5 px-3.5 py-1.5 text-[12px]">
			<PortNumber port={port} />
			<Process port={port} />
			<span className="truncate font-mono text-[11.5px] text-status-ready-text">{local}</span>
			<span className="flex justify-end gap-0.5">
				<button
					type="button"
					aria-label={t("Open {{local}} in the browser", { local: local })}
					className={ICON_BUTTON}
					onClick={() => actions.open(port, forward.local_port)}
				>
					<ExternalLink aria-hidden size={12} strokeWidth={2.2} />
				</button>
				<button
					type="button"
					aria-label={t("Stop forwarding {{port}}", {
						port: formatNumber(port.port, { useGrouping: false }),
					})}
					className={ICON_BUTTON}
					disabled={actions.busy(port.id)}
					onClick={() => {
						// Once stopped, the port's prompt stays away. This list still has it.
						dismissPort(port.id);
						actions.stop(port, forward.id);
					}}
				>
					<Unplug aria-hidden size={12} strokeWidth={2.2} />
				</button>
			</span>
			{forward.local_port !== port.port ? (
				<span className="col-start-2 col-end-[-1] text-[11px] text-status-busy-text">
					{t("{{port}} is busy on this Mac, forwarded to {{local_port}}", {
						port: formatNumber(port.port, { useGrouping: false }),
						local_port: formatNumber(forward.local_port, { useGrouping: false }),
					})}
				</span>
			) : null}
			<Failure text={actions.failure(port.id)} />
		</li>
	);
};

const DeviceIcon = ({ remote }: { remote: boolean }) => {
	const Icon = remote ? Server : Laptop;
	return <Icon size={12} strokeWidth={2} aria-hidden className="shrink-0" />;
};

/**
 * The header's place for an agent, which opens its workspace's ports. A
 * `folded` place shows the device's icon for its name and container, with
 * both in a tooltip.
 */
export const WorkspacePorts = ({
	agent,
	workspace,
	open,
	onOpenChange,
	folded = false,
}: {
	agent: Agent;
	workspace: Workspace;
	/** Set to open it from elsewhere, such as the activity dot. */
	open?: boolean;
	onOpenChange?: (open: boolean) => void;
	folded?: boolean;
}) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const now = useNow(1000);
	const actions = usePortActions();
	const remote = isRemoteDevice(view, workspace.device);
	const forwards = forwardsPorts(view, workspace.device);
	const offline = remote && isUnreachable(view, workspace.device, now);
	const device = deviceName(view, workspace.device);
	const place = placeName(workspace);
	const { location } = workspace;
	const container = location.type === "container";

	const ports = workspacePorts(view, workspace.id);
	const forwarded = ports.flatMap((port) => {
		const forward = forwardOf(view, port.id);
		return forward ? [{ port, forward }] : [];
	});
	const rest = ports.filter((port) => !forwarded.some((f) => f.port === port));
	const listening = rest.filter((port) => port.agent !== null);
	const others = rest.filter((port) => port.agent === null);
	const count = forwarded.length;

	const link = linkForDevice(view, workspace.device);
	const network = link?.network ? view.tables.network.get(link.network) : undefined;
	const where = remote
		? t("On {{device}}, over {{value2}}.", {
				device: device,
				value2: network?.kind.type === "tailscale" ? "Tailscale" : "SSH",
			})
		: t("On this Mac.");
	const agents = all(view, "agent").filter(
		(a) => a.workspace === workspace.id && a.parent === null
	).length;
	const label = [
		t("Workspace {{place}} on {{device}}", { place: place, device: device }),
		count > 0
			? t("{{countText}} forwarded ports", { count, countText: formatNumber(count) })
			: null,
	]
		.filter(Boolean)
		.join(", ");

	return (
		<Popover open={open} onOpenChange={onOpenChange}>
			<PopoverTrigger
				aria-label={label}
				title={folded ? label : undefined}
				className="inline-flex h-[26px] min-w-0 shrink cursor-pointer items-center gap-1.5 overflow-hidden rounded-md px-1.5 text-secondary-foreground outline-none hover:bg-secondary focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-secondary data-popup-open:shadow-[inset_0_0_0_1px_var(--input)]"
			>
				<AgentConnectionDot agent={agent} view={view} now={now} />
				{folded ? (
					<DeviceIcon remote={remote} />
				) : (
					<span className="max-w-32 truncate">{device}</span>
				)}
				{container && !folded ? (
					<span className="inline-flex max-w-32 min-w-0 items-center gap-[5px] text-muted-foreground">
						<Box size={12} strokeWidth={2} aria-hidden className="shrink-0" />
						<span className="truncate">{location.instance}</span>
					</span>
				) : null}
				{count > 0 ? (
					<span
						aria-hidden
						className="inline-flex h-[18px] shrink-0 items-center gap-1 rounded-[5px] bg-status-ready/14 px-1.5 text-[11px] font-semibold text-status-ready-text tabular-nums"
					>
						<Plug size={10} strokeWidth={2.4} />
						{formatNumber(count)}
					</span>
				) : null}
			</PopoverTrigger>
			<PopoverPopup side="bottom" align="end" sideOffset={6} className="w-[404px] p-0">
				<div className="px-3.5 pt-3 pb-2.5">
					<div className="flex items-center gap-2">
						{container ? (
							<Box
								aria-hidden
								size={13}
								strokeWidth={2}
								className="text-muted-foreground"
							/>
						) : (
							<Folder
								aria-hidden
								size={13}
								strokeWidth={2}
								className="text-muted-foreground"
							/>
						)}
						<PopoverTitle className="truncate text-[13px] font-[650] text-foreground-strong">
							{place}
						</PopoverTitle>
						<span className="text-[11px] text-muted-foreground">
							{container ? t("Incus container") : t("Folder")}
						</span>
					</div>
					<p className="mt-1.5 text-[11.5px] text-muted-foreground">
						{t("{{where}} {{countText}} agents work here.", {
							where,
							count: agents,
							countText: formatNumber(agents),
						})}
						{remote && !forwards
							? t(" {{device}} doesn't forward ports.", { device: device })
							: null}
						{forwards && offline
							? t(
									" {{device}} can't be reached right now. Forwards pick up again once it's back.",
									{ device: device }
								)
							: null}
					</p>
				</div>
				{forwarded.length > 0 ? (
					<Section title={t("Forwarded ports")}>
						{forwarded.map(({ port, forward }) => (
							<ForwardedRow
								key={port.id}
								port={port}
								forward={forward}
								actions={actions}
							/>
						))}
					</Section>
				) : null}
				{listening.length > 0 ? (
					<Section title={remote ? t("Listening, not forwarded") : t("Listening")}>
						{listening.map((port) => (
							<ListeningRow
								key={port.id}
								port={port}
								remote={remote}
								forwards={forwards && !offline}
								actions={actions}
							/>
						))}
					</Section>
				) : null}
				{others.length > 0 ? (
					<Section title={t("Other ports")}>
						{others.map((port) => (
							<ListeningRow
								key={port.id}
								port={port}
								remote={remote}
								forwards={forwards && !offline}
								quiet
								actions={actions}
							/>
						))}
					</Section>
				) : null}
				{ports.length === 0 ? (
					<p className="border-t border-secondary px-3.5 py-3 text-[12px] text-muted-foreground">
						{t("Nothing listens on a port here yet.")}
					</p>
				) : null}
				<Running agent={agent.id} view={view} live={!offline} />
			</PopoverPopup>
		</Popover>
	);
};
