// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the rest of the app may use of the terminal module. The chat's "You
// ran" cards, the output pill and the code blocks' Run menu reach terminals
// only through these, so what draws a terminal can change behind them.

import { createElement, type ReactElement, useMemo } from "react";

import { useServices } from "@/app/services";
import type { AgentId, DeviceId, TerminalId } from "@maxhq/shared/bindings";
import type { InputOptions } from "@maxhq/shared/terminal/stream";

import { openAgentShell, pasteIntoDrawer, showInDrawer } from "./actions";
import { cellSize, registryOf } from "./live";
import { LiveTerminalHost } from "./live-terminal";
import { commandText } from "./term-view";

/** Which terminal `LiveTerminal` shows, and how. */
export type LiveTerminalProps = {
	readonly device: DeviceId;
	readonly terminal: TerminalId;
	/**
	 * The most rows it takes. It takes fewer while the output is shorter, and
	 * fills its parent when left out.
	 */
	readonly maxRows?: number;
	/** Whether keys typed in it go to the terminal. */
	readonly interactive: boolean;
	readonly className?: string;
};

/**
 * One terminal's live output. The terminal lives on outside React, so a row
 * that scrolls away and back keeps its screen.
 */
export const LiveTerminal = ({
	terminal,
	maxRows,
	interactive,
	className,
}: LiveTerminalProps): ReactElement | null =>
	createElement(LiveTerminalHost, { terminal, maxRows, interactive, className });

/** What the chat asks of terminals. */
export type TerminalActions = {
	/**
	 * Types `data` into a terminal. MaxHQ never keeps or logs it. With
	 * `secret`, from a field that hides what you type, the device records
	 * only that a secret went, when it knows how.
	 */
	readonly sendInput: (
		device: DeviceId,
		terminal: TerminalId,
		data: string,
		options?: InputOptions
	) => void;
	/**
	 * A live terminal's last `lines` lines as plain text, and how many lines
	 * it printed in all, or null when this window doesn't hold it.
	 */
	readonly text: (
		terminal: TerminalId,
		lines: number
	) => { readonly text: string; readonly total: number } | null;
	/**
	 * What the command whose output starts at `start` in a terminal printed
	 * so far, by its shell's marks, and how many lines that is. Null when
	 * this window doesn't hold the terminal or that command, or the device
	 * hasn't placed its start.
	 */
	readonly runningText: (
		terminal: TerminalId,
		start: number | null
	) => { readonly text: string; readonly total: number } | null;
	/** One cell of the terminal font, in CSS pixels. */
	readonly cellSize: () => { readonly width: number; readonly height: number };
	/**
	 * Opens the agent's drawer and types `text` into its active tab without
	 * Enter, opening a shell when there's none.
	 */
	readonly pasteIntoDrawer: (agent: AgentId, text: string) => void;
	/** Opens the agent's drawer on this terminal's tab. */
	readonly showInDrawer: (agent: AgentId, terminal: TerminalId) => void;
	/**
	 * Opens the agent's drawer on its Agent shell, focused, starting one
	 * when there's none.
	 */
	readonly openAgentShell: (agent: AgentId) => void;
};

const encoder = new TextEncoder();

/** The terminal calls, for this window. */
export const useTerminalActions = (): TerminalActions => {
	const services = useServices();
	return useMemo(
		() => ({
			sendInput: (_device, terminal, data, options) =>
				services.source.terminalStream?.input(terminal, encoder.encode(data), options),
			text: (terminal, lines) => registryOf(services)?.view(terminal)?.text(lines) ?? null,
			runningText: (terminal, start) => {
				const view = start === null ? undefined : registryOf(services)?.view(terminal);
				return view && start !== null ? commandText(view, start) : null;
			},
			cellSize,
			pasteIntoDrawer: (agent, text) => void pasteIntoDrawer(services, agent, text),
			showInDrawer: (agent, terminal) => showInDrawer(services, agent, terminal),
			openAgentShell: (agent) => void openAgentShell(services, agent),
		}),
		[services]
	);
};

export { useShownInPanel } from "./shown";
