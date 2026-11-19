// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The terminal calls of the sample data and of tests. In the sample data an
// open adds a terminal in the agent's folder with a made-up shell or
// command behind it (see `mock-terminal-stream.ts`), and an interrupt sends
// it Ctrl-C. A command you ran with `!` adds a "You ran" card to the
// agent's timeline, which gets how it ended and what it printed, as `maxd`
// does.

import type { AgentId, ItemId, Terminal, TerminalId, TerminalStatus } from "../bindings";
import { runsCommand } from "../terminal/drawer";
import type { TerminalCalls } from "../terminal/requests";
import type { TerminalStream } from "../terminal/stream";
import type { FleetContext } from "./mock-fleet";
import { mockTerminalStream } from "./mock-terminal-stream";
import type { MockTimelines } from "./mock-timeline";
import type { ActionResult, CommandResult } from "./source";

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const OK: CommandResult = { status: "ok", data: null };

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

const notFound = (what: string) => ({
	status: "error" as const,
	error: { kind: "not_found" as const, what },
});

/** What a `maxd` without terminals says, as a device on protocol 44 does. */
export const NO_TERMINALS = "Terminals aren't available on this maxd yet.";

/** The terminal calls with no terminals behind them. */
export const noTerminals: TerminalCalls = {
	openTerminal: async () => refused(NO_TERMINALS),
	attachTerminal: async () => OK,
	detachTerminal: async () => OK,
	resizeTerminal: async () => OK,
	interruptTerminal: async () => refused(NO_TERMINALS),
	closeTerminal: async () => refused(NO_TERMINALS),
	renameTerminal: async () => refused(NO_TERMINALS),
	moveTerminal: async () => refused(NO_TERMINALS),
};

/** What a device says to a close without force while a command runs. */
export const STILL_RUNNING = "Still running. Close anyway?";

/** The signal a terminal that closes sends what runs in it. */
const SIGHUP = 1;

/** The ID of a `!` run's card, as `maxd` names it. */
const runItem = (terminal: TerminalId): ItemId => `run-${terminal}`;

/** The tab's name until the shell or you set one. */
const defaultTitle = (terminal: Pick<Terminal, "kind">) =>
	terminal.kind.type === "shell" ? "zsh" : terminal.kind.command;

/**
 * The terminal calls over the sample entities, and their bytes. With
 * `runs`, a `!` run shows as a card in its agent's timeline.
 */
export const mockTerminals = (
	context: FleetContext,
	runs?: Pick<MockTimelines, "post" | "amend">
): TerminalCalls & { readonly terminalStream: TerminalStream } => {
	const { now, change } = context;
	let opened = 0;
	const find = (id: TerminalId): Terminal | undefined => {
		for (const e of context.entities()) {
			if (e.entity === "terminal" && e.id === id) return e;
		}
		return undefined;
	};
	const edit = (id: TerminalId, next: (terminal: Terminal) => Terminal) =>
		change((base) =>
			base.map((e) => (e.entity === "terminal" && e.id === id ? { ...e, ...next(e) } : e))
		);
	/** The device and folder an agent runs in. */
	const placeOf = (agent: AgentId) => {
		let workspace: string | undefined;
		for (const e of context.entities()) {
			if (e.entity === "agent" && e.id === agent) workspace = e.workspace;
		}
		for (const e of context.entities()) {
			if (e.entity === "workspace" && e.id === workspace)
				return { device: e.device, cwd: e.location.path };
		}
		return undefined;
	};
	/** Ends a `!` run's card as `process` says, with what it printed. */
	const endRun = (terminal: Terminal, process: TerminalStatus) => {
		if (terminal.kind.type !== "run" || process.state !== "exited") return;
		const shown = printed(terminal.id);
		const at = now();
		runs?.amend(terminal.agent, runItem(terminal.id), (item) =>
			item.kind === "user_run"
				? {
						...item,
						status: process.code === 0 ? "completed" : "failed",
						completed_at: at,
						process,
						duration_ms: at - (item.started_at ?? at),
						lines: shown?.lines ?? 0,
						output: shown?.text ?? "",
						output_truncated: shown?.truncated ?? false,
					}
				: item
		);
	};
	/** Answers for a terminal that's there, or says it's gone. */
	const withTerminal =
		<A extends unknown[]>(act: (terminal: Terminal, ...args: A) => ActionResult) =>
		async (id: TerminalId, ...args: A): Promise<ActionResult> => {
			const terminal = find(id);
			return terminal ? act(terminal, ...args) : notFound(`terminal ${id}`);
		};
	const { start, interrupt, end, named, printed, ...terminalStream } = mockTerminalStream({
		find,
		edit,
		now,
		exited: (id) => {
			const terminal = find(id);
			if (terminal) endRun(terminal, terminal.status);
		},
	});
	return {
		terminalStream,
		openTerminal: async (agent, kind, { cols, rows }) => {
			const place = placeOf(agent);
			if (!place) return notFound(`agent ${agent}`);
			opened += 1;
			const terminal: Terminal = {
				id: `0199a7c2-0000-7000-8000-${opened.toString(16).padStart(12, "0")}`,
				agent,
				device: place.device,
				kind,
				moved: false,
				title: defaultTitle({ kind }),
				cwd: place.cwd,
				status: { state: "running" },
				busy: false,
				password_prompt: false,
				started_at: now(),
				cols,
				rows,
			};
			change((base) => [...base, { entity: "terminal", ...terminal }]);
			if (kind.type === "run")
				runs?.post(
					agent,
					{
						kind: "user_run",
						terminal: terminal.id,
						command: kind.command,
						cwd: place.cwd,
						device: place.device,
						private: kind.private,
						process: { state: "running" },
						duration_ms: null,
						lines: 0,
						output: "",
						output_truncated: false,
						moved: false,
					},
					"running",
					runItem(terminal.id)
				);
			start(terminal);
			return { status: "ok", data: { status: "terminal_opened", terminal: terminal.id } };
		},
		attachTerminal: async (id) => (find(id) ? OK : notFound(`terminal ${id}`)),
		detachTerminal: async () => OK,
		resizeTerminal: async (id, size) => {
			if (!find(id)) return notFound(`terminal ${id}`);
			terminalStream.resize(id, size);
			return OK;
		},
		interruptTerminal: withTerminal((terminal) => {
			// A shell takes Ctrl-C and goes on; a command ends.
			if (terminal.status.state === "running") interrupt(terminal.id);
			return DONE;
		}),
		closeTerminal: withTerminal((terminal, force = false) => {
			if (!force && runsCommand(terminal)) return refused(STILL_RUNNING);
			if (terminal.status.state === "running")
				endRun(terminal, { state: "exited", code: null, signal: SIGHUP });
			end(terminal.id);
			change((base) =>
				base.filter((e) => !(e.entity === "terminal" && e.id === terminal.id))
			);
			return DONE;
		}),
		renameTerminal: withTerminal((terminal, title: string) => {
			named(terminal.id, title.trim() !== "");
			edit(terminal.id, (t) => ({ ...t, title: title.trim() || defaultTitle(t) }));
			return DONE;
		}),
		moveTerminal: withTerminal((terminal) => {
			if (terminal.kind.type === "shell") return refused("A shell is already in the drawer.");
			if (terminal.moved) return refused("This command is already in the drawer.");
			edit(terminal.id, (t) => ({ ...t, moved: true }));
			runs?.amend(terminal.agent, runItem(terminal.id), (item) =>
				item.kind === "user_run" ? { ...item, moved: true } : item
			);
			return DONE;
		}),
	};
};
