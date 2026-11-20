// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The port prompts as a stack: the ports an agent opened that still wait
// on you, newest first. The newest shows in full and the rest fold into a
// strip under it, which names the first few. A port leaves the stack once
// it's forwarded or dismissed, and a device that can't take an offer now
// has none in it. At most one notice says a forward stopped. The Port
// prompts setting, kept per client, turns the stack off.

import type { AgentId, DeviceId, ListeningPort, PortId, Timestamp } from "../bindings";
import { all, type ViewState } from "../view/replica";

import {
	type ClosedForward,
	forwardsPorts,
	isRemoteDevice,
	isUnreachable,
	listedPorts,
} from "./select";

/** How many of the rest the strip names. */
export const STACK_CHIPS = 4;

/**
 * Ports newest first: the one first seen last leads, and of two seen at
 * once, the one maxd listed later.
 */
export const newestFirst = (ports: readonly ListeningPort[]): ListeningPort[] =>
	ports
		.map((port, at) => ({ port, at }))
		.sort((a, b) => b.port.first_seen - a.port.first_seen || b.at - a.at)
		.map(({ port }) => port);

/**
 * Whether a device's ports get offers now. A port on this Mac opens in the
 * browser. One elsewhere needs a device that forwards ports and can be
 * reached, since a forward could only fail.
 */
export const offersPorts = (view: ViewState, device: DeviceId, now: Timestamp): boolean =>
	!isRemoteDevice(view, device) ||
	(forwardsPorts(view, device) && !isUnreachable(view, device, now));

/** The agent's open ports with no forward that you haven't dismissed, newest first. */
export const waitingPorts = (
	view: ViewState,
	agent: AgentId,
	dismissed: ReadonlySet<PortId>
): ListeningPort[] => {
	const forwarded = new Set(all(view, "forward").map((forward) => forward.port));
	return newestFirst(
		listedPorts(view, agent).filter(
			(port) => !dismissed.has(port.id) && !forwarded.has(port.id)
		)
	);
};

/** The ports that wait on you, as the stack shows them. */
export type PortStack = {
	/** The port that started listening last, shown in full. */
	readonly newest: ListeningPort;
	/** The others, newest first. */
	readonly rest: readonly ListeningPort[];
	/** The first of the rest, which the strip names. */
	readonly chips: readonly ListeningPort[];
	/** How many of the rest the strip leaves unnamed, for its "+N". */
	readonly more: number;
};

/** The stack for the waiting ports, newest first, or null when none wait. */
export const portStack = (waiting: readonly ListeningPort[]): PortStack | null => {
	const [newest, ...rest] = waiting;
	if (!newest) return null;
	const chips = rest.slice(0, STACK_CHIPS);
	return { newest, rest, chips, more: rest.length - chips.length };
};

/**
 * The latest notice that one of the agent's forwards stopped, or null. A
 * port that opened again gets a new offer, which stands in for its notice.
 */
export const latestNotice = (
	closed: readonly ClosedForward[],
	agent: AgentId,
	open: readonly ListeningPort[]
): ClosedForward | null =>
	closed
		.filter(
			(notice) => notice.agent === agent && !open.some((port) => port.port === notice.port)
		)
		.at(-1) ?? null;

/** Reads the kept Port prompts setting: on unless it was turned off. */
export const readPortPrompts = (raw: string | null): boolean => raw !== "off";

/** The Port prompts setting as kept. */
export const writePortPrompts = (shown: boolean): string => (shown ? "on" : "off");
