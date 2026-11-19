// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the phone knows about an agent's ports. The view lists every
// computer's forwards and can't say which are this phone's, so the phone
// keeps the ones it opened here, from what each Forward gave back. One
// counts while the view still has it, found by its ID or else by the port
// it listens on, which a reconnect keeps, or while the device hasn't named
// it yet. A forward a Mac opened only reaches that Mac, so the phone offers
// its own. The rest sorts ports into the screen's sections and the cards
// above the composer, and answers what the agent's More sheet asks.

import { create } from "zustand";

import type {
	Agent,
	AgentId,
	Forward,
	ForwardId,
	ListeningPort,
	PortId,
	Workspace,
} from "@maxhq/shared/bindings";
import type { ForwardHooks, OpenedForward } from "@maxhq/shared/ports/actions";
import { agentPorts, type ClosedForward, workspacePorts } from "@maxhq/shared/ports/select";
import { all, type ViewState } from "@maxhq/shared/view/replica";
import type { ViewStore } from "@maxhq/shared/view/store";

/** A forward this phone opened. */
export type MyForward = {
	readonly port: PortId;
	/** The agent whose process held the port, for the notice once it closes. */
	readonly agent: AgentId | null;
	/** The port's number in the workspace. */
	readonly number: number;
	/** The device's ID for it, or null until the device answered. */
	readonly forward: ForwardId | null;
	/** The port it listens on, on this phone. */
	readonly local: number;
};

type MyForwards = { readonly mine: ReadonlyMap<PortId, MyForward> };

/** The forwards this phone opened, for as long as the app runs, as its core's do. */
export const useMyForwards = create<MyForwards>(() => ({ mine: new Map() }));

/** Remembers a forward this phone just opened to `port`. */
export const keepForward = (port: ListeningPort, opened: OpenedForward) =>
	useMyForwards.setState(({ mine }) => ({
		mine: new Map(mine).set(port.id, {
			port: port.id,
			agent: port.agent,
			number: port.port,
			forward: opened.forward,
			local: opened.local_port,
		}),
	}));

/** Forgets the phone's forward to `port`, once it's closed. */
export const dropForward = (port: PortId) => {
	const { mine } = useMyForwards.getState();
	if (!mine.has(port)) return;
	const next = new Map(mine);
	next.delete(port);
	useMyForwards.setState({ mine: next });
};

/** What the phone remembers as its forwards open and close. */
export const PHONE_HOOKS: ForwardHooks = {
	forwarded: keepForward,
	stopped: (port) => dropForward(port.id),
};

/** Forgets every forward, for tests. */
export const clearMyForwards = () => useMyForwards.setState({ mine: new Map() }, true);

/** The phone's forward to a port as it stands: what Stop closes, and where it listens here. */
export type PhoneForward = {
	/** What Stop forwarding closes, or null while the device hasn't named it. */
	readonly forward: ForwardId | null;
	readonly local: number;
};

/**
 * The view's forward for `mine`: the one with its ID, or else the newest
 * that listens on the same port here. A Mac that forwards the same port to
 * the same number opened its forward first.
 */
const findForward = (view: ViewState, mine: MyForward): Forward | undefined => {
	const forwards = all(view, "forward").filter((forward) => forward.port === mine.port);
	return (
		forwards.find((forward) => forward.id === mine.forward) ??
		forwards
			.filter((forward) => forward.local_port === mine.local)
			.reduce<Forward | undefined>(
				(newest, forward) =>
					newest === undefined || forward.opened_at > newest.opened_at ? forward : newest,
				undefined
			)
	);
};

/** The phone's forward in `mine` as the view has it now, or null once it's gone. */
export const myForward = (view: ViewState, mine: MyForward | undefined): PhoneForward | null => {
	if (!mine) return null;
	const found = findForward(view, mine);
	if (found) return { forward: found.id, local: found.local_port };
	return mine.forward === null ? { forward: null, local: mine.local } : null;
};

/**
 * Gives the forwards the device hadn't named yet the ID the view now has
 * for them, so one that closes later counts as gone. Returns `mine` when
 * nothing changed.
 */
export const nameForwards = (
	view: ViewState,
	mine: ReadonlyMap<PortId, MyForward>
): ReadonlyMap<PortId, MyForward> => {
	let named: Map<PortId, MyForward> | null = null;
	for (const kept of mine.values()) {
		if (kept.forward !== null) continue;
		const found = findForward(view, kept);
		if (!found) continue;
		named ??= new Map(mine);
		named.set(kept.port, { ...kept, forward: found.id });
	}
	return named ?? mine;
};

/**
 * Names the phone's forwards as the view learns them, until the returned
 * function is called. Several watchers on one view do the same work once.
 */
export const watchMyForwards = (view: ViewStore): (() => void) => {
	const name = (state: ViewState) => {
		const { mine } = useMyForwards.getState();
		const named = nameForwards(state, mine);
		if (named !== mine) useMyForwards.setState({ mine: named });
	};
	name(view.getState().view);
	return view.subscribe((state, previous) => {
		if (state.view !== previous.view) name(state.view);
	});
};

/** A port with the phone's forward to it, if it has one. */
export type PortRow = { readonly port: ListeningPort; readonly forward: PhoneForward | null };

/** The screen's sections, each lowest port first. */
export type PortSections = {
	/** The ports this phone forwards. */
	readonly forwarded: readonly (PortRow & { readonly forward: PhoneForward })[];
	/** The ports an agent here opened, not forwarded to this phone. */
	readonly listening: readonly ListeningPort[];
	/** The ports no agent opened. */
	readonly others: readonly ListeningPort[];
};

/** Sorts the ports that listen in `workspace` into the screen's sections. */
export const portSections = (
	view: ViewState,
	workspace: Workspace,
	mine: ReadonlyMap<PortId, MyForward>
): PortSections => {
	const rows = workspacePorts(view, workspace.id).map((port): PortRow => ({
		port,
		forward: myForward(view, mine.get(port.id)),
	}));
	const rest = rows.filter((row) => row.forward === null).map((row) => row.port);
	return {
		forwarded: rows.flatMap((row) =>
			row.forward ? [{ port: row.port, forward: row.forward }] : []
		),
		listening: rest.filter((port) => port.agent !== null),
		others: rest.filter((port) => port.agent === null),
	};
};

/**
 * The cards above the composer for `agent`'s ports: each one this phone
 * forwards, and an offer for the others unless you dismissed it or the
 * device can't take a forward now.
 */
export const offerCards = (
	view: ViewState,
	agent: AgentId,
	mine: ReadonlyMap<PortId, MyForward>,
	dismissed: ReadonlySet<PortId>,
	/** Whether a forward could open now: the device forwards ports and can be reached. */
	offers: boolean
): PortRow[] =>
	agentPorts(view, agent).flatMap((port): PortRow[] => {
		const forward = myForward(view, mine.get(port.id));
		if (forward) return [{ port, forward }];
		if (!offers || dismissed.has(port.id)) return [];
		return [{ port, forward: null }];
	});

/**
 * The notices about `agent`'s forwards that stopped because their port
 * closed, kept to the ones this phone opened. A port that opened again
 * gets a new offer, which stands in for its notice.
 */
export const myNotices = (
	closed: readonly ClosedForward[],
	mine: ReadonlyMap<PortId, MyForward>,
	agent: AgentId,
	open: readonly ListeningPort[]
): ClosedForward[] =>
	closed.filter(
		(notice) =>
			notice.agent === agent &&
			!open.some((port) => port.port === notice.port) &&
			[...mine.values()].some(
				(kept) =>
					kept.agent === notice.agent &&
					kept.number === notice.port &&
					kept.local === notice.localPort
			)
	);

/** The phone's forwards a notice is about, to forget along with it. */
export const noticeForwards = (
	notice: ClosedForward,
	mine: ReadonlyMap<PortId, MyForward>
): PortId[] =>
	[...mine.values()]
		.filter((kept) => kept.agent === notice.agent && kept.number === notice.port)
		.map((kept) => kept.port);

/**
 * The address the phone shows a forwarded port at. It names 127.0.0.1,
 * which the forward listens on along with ::1, because iOS doesn't hold a
 * plain http address that's an IP to its secure-connection rules.
 */
export const pageUrl = (local: number): string => `http://127.0.0.1:${local}`;

// An address on this phone with its port, without a URL parser the app may lack.
const HERE = /^(?:http|ws):\/\/(?:127\.0\.0\.1|localhost|\[::1\]):(\d+)(?:[/?#]|$)/i;

/** Whether `url` is on the forward at `local`, so the page's own view opens it. */
export const onForward = (url: string, local: number): boolean => {
	if (url === "about:blank") return true;
	const port = HERE.exec(url)?.[1];
	return port !== undefined && Number(port) === local;
};

/** Whether a link off the forward is one the browser can open. */
export const opensOutside = (url: string): boolean => /^https?:\/\//i.test(url);

/**
 * Whether the agent's More sheet offers Ports and processes: its workspace
 * is in the view, so there are ports to list and a device to ask.
 */
export const showsPorts = (view: ViewState, agent: Agent): boolean =>
	view.tables.workspace.has(agent.workspace);

/** How many ports the agent's processes listen on now. */
export const openPortCount = (view: ViewState, agent: AgentId): number => {
	let count = 0;
	for (const port of view.tables.listening_port.values())
		if (port.agent === agent && port.gone_since === null) count += 1;
	return count;
};
