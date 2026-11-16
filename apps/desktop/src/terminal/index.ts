// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the rest of the app may use of the terminal module. The chat's "You
// ran" cards, the output pill and the code blocks' Run menu reach terminals
// only through these, so what draws a terminal can change behind them.

import type { ReactElement } from "react";

import type { AgentId, DeviceId, TerminalId } from "@maxhq/shared/bindings";

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
export const LiveTerminal: (props: LiveTerminalProps) => ReactElement | null = () => null;

/** What the chat asks of terminals. */
export type TerminalActions = {
	/** Types `data` into a terminal. MaxHQ never keeps or logs it. */
	readonly sendInput: (device: DeviceId, terminal: TerminalId, data: string) => void;
	/**
	 * A live terminal's last `lines` lines as plain text, and how many lines
	 * it printed in all, or null when this window doesn't hold it.
	 */
	readonly text: (
		terminal: TerminalId,
		lines: number
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
};

// Stand-ins until the terminal module fills these in.
const STAND_INS: TerminalActions = {
	sendInput: () => {},
	text: () => null,
	cellSize: () => ({ width: 8, height: 17 }),
	pasteIntoDrawer: () => {},
	showInDrawer: () => {},
};

/** The terminal calls, for this window. */
export const useTerminalActions = (): TerminalActions => STAND_INS;
