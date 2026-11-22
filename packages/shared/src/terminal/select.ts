// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Reading terminals out of the view: an agent's drawer tabs, one terminal by
// its id, whether a command waits on you, and whether a device's `maxd` has
// terminals at all.

import { type AgentId, type DeviceId, TERMINALS_SINCE, type TerminalId } from "../bindings";
import { all, type EntityOf, type ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

/** A terminal as the view holds it. */
export type TerminalEntity = EntityOf<"terminal">;

/**
 * Whether the terminal is a tab in its agent's drawer: a shell, the
 * session your `!` commands share, or a command you ran with `!` and moved
 * there. Any other command is a card in the timeline.
 */
export const inDrawer = (terminal: TerminalEntity): boolean =>
	terminal.kind.type === "shell" || terminal.kind.type === "session" || terminal.moved;

/** The agent's drawer tabs: its session first, then the rest in the order they started. */
export const terminalsOf = (view: ViewState, agent: AgentId): TerminalEntity[] =>
	all(view, "terminal")
		.filter((terminal) => terminal.agent === agent && inDrawer(terminal))
		.sort(
			(a, b) =>
				Number(b.kind.type === "session") - Number(a.kind.type === "session") ||
				a.started_at - b.started_at ||
				(a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
		);

/**
 * Whether a command you ran with `!`, on its own or in your session, waits
 * on you: `maxd` saw it go quiet while it reads what you type, or it asks
 * for a password. A `maxd` older than protocol 59 says only the second.
 */
export const waitsForYou = (terminal: TerminalEntity): boolean =>
	terminal.kind.type !== "shell" &&
	terminal.status.state === "running" &&
	(terminal.waiting === true || terminal.password_prompt);

/** Whether any command you ran with `!` for the agent waits on you, so its row shows Input. */
export const commandWaits = (view: ViewState, agent: AgentId): boolean =>
	all(view, "terminal").some((terminal) => terminal.agent === agent && waitsForYou(terminal));

/** The terminal with this id, if the view has it. */
export const terminalById = (view: ViewState, id: TerminalId): TerminalEntity | undefined =>
	view.tables.terminal.get(id);

/**
 * Whether the device's `maxd` opens terminals: it says so, and speaks a
 * protocol that has them. A device with no link, or one whose version isn't
 * known yet, has none.
 */
export const supportsTerminals = (view: ViewState, device: DeviceId): boolean => {
	const link = linkForDevice(view, device);
	const protocol = link?.protocol ?? null;
	return (
		link?.capabilities?.terminals === true && protocol !== null && protocol >= TERMINALS_SINCE
	);
};
