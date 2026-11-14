// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The cards above the composer for ports the agent's processes listen on,
// per N-6. A port on another device offers a forward to this Mac, and once
// forwarded the card says where it landed. A port on this Mac offers to
// open it in the browser, since it needs no forward. A device that doesn't
// forward ports gets no offer, and neither does one that can't be reached,
// where a forward could only fail. Its forwards stay, saying so. When a
// forwarded port closes, a quiet line says the forward stopped.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { Plug, X } from "lucide-react";
import { type ReactNode, useEffect } from "react";

import { useServices, useView } from "@/app/services";
import { Spinner } from "@/agent-view/icons";
import type { Agent, Forward, ListeningPort } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";
import { useNow } from "@maxhq/shared/lib/clock";
import { deviceName } from "@maxhq/shared/view/select";

import { type PortActions, usePortActions } from "./actions";
import {
	agentPorts,
	type ClosedForward,
	forwardOf,
	forwardsPorts,
	isRemoteDevice,
	isUnreachable,
	subjectName,
} from "@maxhq/shared/ports/select";
import { dismissClosed, dismissPort, usePorts, watchClosed } from "@maxhq/shared/ports/store";

const BUTTON =
	"inline-flex h-[26px] shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-2.5 text-[12px] whitespace-nowrap outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:opacity-60";

const PRIMARY = cn(
	BUTTON,
	"bg-primary font-semibold text-primary-foreground shadow-lit hover:bg-primary/90"
);

const SECONDARY = cn(
	BUTTON,
	"border border-input bg-secondary text-foreground hover:bg-hover-card"
);

const GHOST = cn(BUTTON, "text-secondary-foreground hover:bg-border hover:text-foreground");

const Code = ({ children }: { children: ReactNode }) => {
	useI18n();
	return <code className="font-mono text-[11px] text-secondary-foreground">{children}</code>;
};

const Card = ({
	port,
	forwarded,
	title,
	detail,
	failure,
	children,
}: {
	port: ListeningPort;
	forwarded: boolean;
	title: ReactNode;
	detail: ReactNode;
	failure: string | null;
	children: ReactNode;
}) => {
	const { t } = useI18n();
	return (
		<section
			aria-label={t("Port {{port}}", {
				port: formatNumber(port.port, { useGrouping: false }),
			})}
			className="flex items-center gap-[11px] rounded-[10px] border border-input bg-card py-2 pr-2.5 pl-3"
		>
			<span
				aria-hidden
				className={cn(
					"grid size-[26px] shrink-0 place-items-center rounded-md",
					forwarded
						? "bg-status-ready/14 text-status-ready-text"
						: "bg-selected text-primary"
				)}
			>
				<Plug size={13} strokeWidth={2.2} />
			</span>
			<div className="flex min-w-0 flex-1 flex-col gap-0.5">
				<p className="truncate text-[12.5px] text-foreground">{title}</p>
				<p className="truncate text-[11.5px] text-muted-foreground">{detail}</p>
				{failure !== null && (
					<p role="alert" className="text-[11.5px] text-status-error-text">
						{failure}
					</p>
				)}
			</div>
			<div className="flex shrink-0 items-center gap-1">{children}</div>
		</section>
	);
};

const Busy = ({ shown }: { shown: boolean }) => {
	useI18n();
	return shown ? (
		<Spinner size={11} className="border-primary-foreground/30 border-t-primary-foreground" />
	) : null;
};

/** What one card needs to say where its port is. */
type Place = {
	/** The project or workspace the port is in, such as "MaxHQ". */
	readonly subject: string;
	/** The device's name, such as "devbox". */
	readonly device: string;
};

const LocalCard = ({
	port,
	place,
	actions,
}: {
	port: ListeningPort;
	place: Place;
	actions: PortActions;
}) => {
	const { t } = useI18n();
	return (
		<Card
			port={port}
			forwarded={false}
			title={t("{{subject}} is listening on port {{port}}.", {
				subject: place.subject,
				port: formatNumber(port.port, { useGrouping: false }),
			})}
			detail={
				<>
					{port.process
						? t("{{name}} on this Mac at ", { name: port.process.name })
						: t("On this Mac at ")}
					<Code>{`localhost:${port.port}`}</Code>
				</>
			}
			failure={actions.failure(port.id)}
		>
			<button
				type="button"
				className={PRIMARY}
				disabled={actions.busy(port.id)}
				onClick={() => actions.open(port, port.port)}
			>
				{t("Open localhost:{{port}}", {
					port: formatNumber(port.port, { useGrouping: false }),
				})}
			</button>
			<button type="button" className={GHOST} onClick={() => dismissPort(port.id)}>
				{t("Dismiss")}
			</button>
		</Card>
	);
};

const OfferCard = ({
	port,
	place,
	actions,
}: {
	port: ListeningPort;
	place: Place;
	actions: PortActions;
}) => {
	const { t } = useI18n();

	const busy = actions.busy(port.id);
	return (
		<Card
			port={port}
			forwarded={false}
			title={t("{{subject}} on {{device}} is listening on port {{port}}.", {
				subject: place.subject,
				device: place.device,
				port: formatNumber(port.port, { useGrouping: false }),
			})}
			detail={
				<>
					{t("Forward to ")}
					<Code>{`localhost:${port.port}`}</Code>?
				</>
			}
			failure={actions.failure(port.id)}
		>
			<button
				type="button"
				className={PRIMARY}
				disabled={busy}
				onClick={() => actions.forward(port)}
			>
				<Busy shown={busy} />
				{t("Forward")}
			</button>
			<button type="button" className={GHOST} onClick={() => dismissPort(port.id)}>
				{t("Dismiss")}
			</button>
		</Card>
	);
};

const ForwardedCard = ({
	port,
	forward,
	place,
	offline,
	actions,
}: {
	port: ListeningPort;
	forward: Forward;
	place: Place;
	/** Whether the device can't be reached right now. */
	offline: boolean;
	actions: PortActions;
}) => {
	const { t } = useI18n();

	const moved = forward.local_port !== port.port;
	const from = t("{{subject}} on {{device}}", { subject: place.subject, device: place.device });
	return (
		<Card
			port={port}
			forwarded
			title={
				moved
					? t("{{port}} is busy on this Mac, forwarded to {{local_port}}", {
							port: formatNumber(port.port, { useGrouping: false }),
							local_port: formatNumber(forward.local_port, { useGrouping: false }),
						})
					: t("Forwarded to localhost:{{local_port}}", {
							local_port: formatNumber(forward.local_port, { useGrouping: false }),
						})
			}
			detail={
				offline
					? t("{{device}} can't be reached. The forward picks up again once it's back.", {
							device: place.device,
						})
					: moved
						? t("Something on this Mac already uses {{port}}.", {
								port: formatNumber(port.port, { useGrouping: false }),
							})
						: port.process
							? t("{{name}} from {{from}}", { name: port.process.name, from: from })
							: t("From {{from}}", { from: from })
			}
			failure={actions.failure(port.id)}
		>
			<button
				type="button"
				className={SECONDARY}
				onClick={() => actions.open(port, forward.local_port)}
			>
				{t("Open in browser")}
			</button>
			<button
				type="button"
				className={GHOST}
				disabled={actions.busy(port.id)}
				onClick={() => {
					// Once stopped, the offer stays away. The workspace popover still lists the port.
					dismissPort(port.id);
					actions.stop(port, forward.id);
				}}
			>
				{t("Stop forwarding")}
			</button>
		</Card>
	);
};

const ClosedLine = ({ notice, device }: { notice: ClosedForward; device: string }) => {
	const { t } = useI18n();
	return (
		<div className="flex items-center gap-2 rounded-[10px] border border-dashed border-input py-1.5 pr-1.5 pl-3 text-[12px] text-muted-foreground">
			<Plug aria-hidden size={12} strokeWidth={2.2} className="shrink-0" />
			<p className="min-w-0 flex-1 truncate">
				{t(
					"Port {{port}} closed on {{device}}. Forwarding to localhost:{{localPort}} stopped.",
					{
						port: formatNumber(notice.port, { useGrouping: false }),
						device: device,
						localPort: formatNumber(notice.localPort, { useGrouping: false }),
					}
				)}
			</p>
			<button
				type="button"
				aria-label={t("Dismiss the notice about port {{port}}", {
					port: formatNumber(notice.port, { useGrouping: false }),
				})}
				onClick={() => dismissClosed(notice.agent, notice.port)}
				className="grid size-5 shrink-0 cursor-pointer place-items-center rounded-[5px] hover:bg-border hover:text-foreground"
			>
				<X aria-hidden size={11} strokeWidth={2.4} />
			</button>
		</div>
	);
};

/** The agent's ports above its composer, or nothing while it listens on none. */
export const PortOffers = ({ agent }: { agent: Agent }) => {
	const { t } = useI18n();

	const { view: store } = useServices();
	const view = useView((s) => s.view);
	const dismissed = usePorts((s) => s.dismissed);
	const closed = usePorts((s) => s.closed);
	const actions = usePortActions();
	const now = useNow(60_000);
	useEffect(() => watchClosed(store), [store]);

	const workspace = view.tables.workspace.get(agent.workspace);
	if (!workspace) return null;
	const remote = isRemoteDevice(view, workspace.device);
	const forwards = forwardsPorts(view, workspace.device);
	const offline = remote && isUnreachable(view, workspace.device, now);
	const place = {
		subject: subjectName(view, workspace),
		device: deviceName(view, workspace.device),
	};
	const ports = agentPorts(view, agent.id);
	const cards = ports.flatMap((port): { port: ListeningPort; forward: Forward | null }[] => {
		const forward = forwardOf(view, port.id);
		if (forward) return [{ port, forward }];
		if (offline || (remote && !forwards)) return [];
		return dismissed.has(port.id) ? [] : [{ port, forward: null }];
	});
	// A port that opened again gets a new offer, which stands in for the notice.
	const notices = closed.filter(
		(notice) => notice.agent === agent.id && !ports.some((port) => port.port === notice.port)
	);
	if (cards.length === 0 && notices.length === 0) return null;

	return (
		<div className="shrink-0 px-7 pb-2">
			<ul aria-label={t("Ports")} className="mx-auto flex max-w-[744px] flex-col gap-1.5">
				{cards.map(({ port, forward }) => (
					<li key={port.id}>
						{forward ? (
							<ForwardedCard
								port={port}
								forward={forward}
								place={place}
								offline={offline}
								actions={actions}
							/>
						) : remote ? (
							<OfferCard port={port} place={place} actions={actions} />
						) : (
							<LocalCard port={port} place={place} actions={actions} />
						)}
					</li>
				))}
				{notices.map((notice) => (
					<li key={`closed-${notice.port}`}>
						<ClosedLine notice={notice} device={deviceName(view, notice.device)} />
					</li>
				))}
			</ul>
		</div>
	);
};
