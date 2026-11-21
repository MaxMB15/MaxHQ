// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The port prompts at the top of an agent's view, per N-6. The port the
// agent's processes started listening on last shows as one card: a port on
// another device offers a forward to this Mac, and one on this Mac offers
// to open it in the browser, since it needs no forward. The rest fold into
// a strip under the card, which names the first few and opens them as a
// list. A device that doesn't forward ports gets no offer, and neither does
// one that can't be reached, where a forward could only fail. Forwarded
// ports leave for the header's ports popover. When a forwarded port
// closes, one quiet line says the forward stopped. The Port prompts setting
// turns all of it off.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { ChevronDown, ChevronUp, Plug, X } from "lucide-react";
import { type ReactNode, useEffect, useId, useState } from "react";

import { useServices, useView } from "@/app/services";
import { Spinner } from "@/agent-view/icons";
import type { Agent, ListeningPort } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";
import { useNow } from "@maxhq/shared/lib/clock";
import { deviceName } from "@maxhq/shared/view/select";

import { type PortActions, usePortActions } from "./actions";
import { GHOST_TEXT_BUTTON, ListeningRow } from "./parts";
import { usePortPrompts } from "./prompts-prefs";
import {
	type ClosedForward,
	isRemoteDevice,
	listedPorts,
	subjectName,
} from "@maxhq/shared/ports/select";
import {
	latestNotice,
	offersPorts,
	type PortStack,
	portStack,
	waitingPorts,
} from "@maxhq/shared/ports/stack";
import {
	dismissAgentClosed,
	dismissPort,
	dismissPorts,
	usePorts,
	watchClosed,
} from "@maxhq/shared/ports/store";

const BUTTON =
	"inline-flex h-[26px] shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-2.5 text-[12px] whitespace-nowrap outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:opacity-60";

const PRIMARY = cn(
	BUTTON,
	"bg-primary font-semibold text-primary-foreground shadow-lit hover:bg-primary/90"
);

const GHOST = cn(BUTTON, "text-secondary-foreground hover:bg-border hover:text-foreground");

const CHIP =
	"inline-flex h-5 cursor-pointer items-center rounded-[5px] border border-track bg-secondary px-1.5 font-mono text-[11px] text-secondary-foreground tabular-nums outline-none hover:border-input hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring";

const Code = ({ children }: { children: ReactNode }) => {
	useI18n();
	return <code className="font-mono text-[11px] text-secondary-foreground">{children}</code>;
};

const Card = ({
	port,
	title,
	detail,
	failure,
	children,
}: {
	port: ListeningPort;
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
			className="flex items-center gap-[11px] py-2 pr-2.5 pl-3"
		>
			<span
				aria-hidden
				className="grid size-[26px] shrink-0 place-items-center rounded-md bg-selected text-primary"
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

/**
 * The line under the card for the ports behind it: how many, the first few
 * by number, and what to do with all of them. A port on this Mac has no
 * Open all, since a port that listens isn't always a web page.
 */
const Strip = ({
	stack,
	remote,
	listed,
	list,
	onList,
	actions,
}: {
	stack: PortStack;
	remote: boolean;
	/** Whether the list of the rest is open. */
	listed: boolean;
	/** The list's element ID. */
	list: string;
	onList: (listed: boolean) => void;
	actions: PortActions;
}) => {
	const { t } = useI18n();
	const count = stack.rest.length;
	const every = [stack.newest, ...stack.rest];
	return (
		<div className="flex flex-wrap items-center gap-2 border-t border-border bg-subpanel py-1.5 pr-2 pl-3 text-[11.5px] text-muted-foreground">
			<span className="text-secondary-foreground">
				{t("{{countText}} more ports", { count, countText: formatNumber(count) })}
			</span>
			{listed ? null : (
				<>
					{stack.chips.map((port) => {
						const number = formatNumber(port.port, { useGrouping: false });
						return (
							<button
								key={port.id}
								type="button"
								aria-label={t("Show port {{port}}", { port: number })}
								className={CHIP}
								onClick={() => onList(true)}
							>
								{number}
							</button>
						);
					})}
					{stack.more > 0 ? <span>+{formatNumber(stack.more)}</span> : null}
				</>
			)}
			<span className="flex-1" />
			{remote ? (
				<button
					type="button"
					className={GHOST_TEXT_BUTTON}
					onClick={() =>
						every
							.filter((port) => !actions.busy(port.id))
							.forEach((port) => actions.forward(port))
					}
				>
					{t("Forward all")}
				</button>
			) : null}
			<button
				type="button"
				className={GHOST_TEXT_BUTTON}
				onClick={() => dismissPorts(every.map((port) => port.id))}
			>
				{t("Dismiss all")}
			</button>
			<button
				type="button"
				aria-expanded={listed}
				aria-controls={list}
				className={cn(GHOST_TEXT_BUTTON, "gap-1")}
				onClick={() => onList(!listed)}
			>
				{listed ? t("Hide") : t("Show all")}
				{listed ? (
					<ChevronUp aria-hidden size={11} strokeWidth={2.2} />
				) : (
					<ChevronDown aria-hidden size={11} strokeWidth={2.2} />
				)}
			</button>
		</div>
	);
};

/** The one quiet line that says a forward stopped, inside the card or on its own. */
const ClosedLine = ({
	notice,
	device,
	joined,
}: {
	notice: ClosedForward;
	device: string;
	/** Whether it sits under the card rather than on its own. */
	joined: boolean;
}) => {
	const { t } = useI18n();
	return (
		<div
			className={cn(
				"flex items-center gap-2 py-1.5 pr-1.5 pl-3 text-[12px] text-muted-foreground",
				joined
					? "border-t border-border bg-subpanel"
					: "rounded-[10px] border border-dashed border-input"
			)}
		>
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
				onClick={() => dismissAgentClosed(notice.agent)}
				className="grid size-5 shrink-0 cursor-pointer place-items-center rounded-[5px] hover:bg-border hover:text-foreground"
			>
				<X aria-hidden size={11} strokeWidth={2.4} />
			</button>
		</div>
	);
};

/**
 * The agent's port prompts under its header, or nothing while no port
 * waits on you, nor a notice.
 */
export const PortOffers = ({ agent }: { agent: Agent }) => {
	const { t } = useI18n();

	const { view: store } = useServices();
	const view = useView((s) => s.view);
	const dismissed = usePorts((s) => s.dismissed);
	const closed = usePorts((s) => s.closed);
	const shown = usePortPrompts();
	const actions = usePortActions();
	const now = useNow(60_000);
	const [listed, setListed] = useState(false);
	const list = useId();
	useEffect(() => watchClosed(store), [store]);

	const workspace = view.tables.workspace.get(agent.workspace);
	if (!shown || !workspace) return null;
	const remote = isRemoteDevice(view, workspace.device);
	const place = {
		subject: subjectName(view, workspace),
		device: deviceName(view, workspace.device),
	};
	const stack = offersPorts(view, workspace.device, now)
		? portStack(waitingPorts(view, agent.id, dismissed))
		: null;
	const notice = latestNotice(closed, agent.id, listedPorts(view, agent.id));
	if (!stack && !notice) return null;
	const noticeLine = notice ? (
		<ClosedLine notice={notice} device={deviceName(view, notice.device)} joined={!!stack} />
	) : null;
	const behind = stack !== null && stack.rest.length > 0;
	const open = behind && listed;

	return (
		<section aria-label={t("Ports")} className="shrink-0 border-b border-row-divider px-7 py-3">
			{stack ? (
				<div className={cn("relative mx-auto max-w-[744px]", behind && !open && "pb-3")}>
					{behind && !open ? (
						<>
							<div
								aria-hidden
								className="absolute inset-x-[22px] top-3 bottom-0 rounded-xl border border-border bg-background"
							/>
							<div
								aria-hidden
								className="absolute inset-x-2.5 top-1.5 bottom-[5px] rounded-xl border border-track bg-subpanel"
							/>
						</>
					) : null}
					<div className="relative overflow-hidden rounded-xl border border-input bg-card shadow-hover">
						{remote ? (
							<OfferCard
								key={stack.newest.id}
								port={stack.newest}
								place={place}
								actions={actions}
							/>
						) : (
							<LocalCard
								key={stack.newest.id}
								port={stack.newest}
								place={place}
								actions={actions}
							/>
						)}
						{behind ? (
							<Strip
								stack={stack}
								remote={remote}
								listed={open}
								list={list}
								onList={setListed}
								actions={actions}
							/>
						) : null}
						{open ? (
							<ul
								id={list}
								aria-label={t("{{countText}} more ports", {
									count: stack.rest.length,
									countText: formatNumber(stack.rest.length),
								})}
								className="max-h-[220px] animate-reveal-down divide-y divide-row-divider overflow-y-auto border-t border-border bg-subpanel"
							>
								{stack.rest.map((port) => (
									<ListeningRow
										key={port.id}
										port={port}
										remote={remote}
										forwards
										actions={actions}
										onDismiss={() => dismissPort(port.id)}
									/>
								))}
							</ul>
						) : null}
						{noticeLine}
					</div>
				</div>
			) : (
				<div className="mx-auto max-w-[744px]">{noticeLine}</div>
			)}
		</section>
	);
};
