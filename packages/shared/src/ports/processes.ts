// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Running list in the ports popover: the processes an agent's device
// found under its holder, split into what the agent started and the
// agent's own CLI with its MCP servers. What the agent started comes as one
// group per command, so a build's dozen compilers fold into one line, with
// the commands that listen on a port first, the rest oldest first, and
// what only keeps the machine awake last. A new command lands at the bottom
// and the rows under the pointer stay put. The phone can show the same list
// from these.

import { formatNumber } from "../i18n";
import type { Activity, AgentId, AgentProcess } from "../bindings";
import type { ViewState } from "../view/replica";
import { nameOf } from "../folders/path";

/** How often the open popover reads the list again. */
export const PROCESS_REFRESH_MS = 2500;

/** How long after Stop a process that still runs is offered Kill. */
export const KILL_AFTER_MS = 3000;

/** Names one process across reads. A pid the system gives out again names another. */
export const processKey = (process: Pick<AgentProcess, "pid" | "start">): string =>
	`${process.pid}:${process.start}`;

/** One row: a process, and whether it sits under the row it belongs to. */
export type ProcessRow = {
	readonly process: AgentProcess;
	/** Whether another process in its part of the list started it. */
	readonly child: boolean;
};

/** One command the agent started: the process heading it and everything under it. */
export type ProcessGroup = {
	readonly head: AgentProcess;
	/** Everything under the head, oldest first. */
	readonly under: readonly AgentProcess[];
	/** The ports anything in it listens on, lowest first. */
	readonly ports: readonly number[];
	/** The memory all of it holds. */
	readonly memory: number;
	/** Whether it only keeps the machine awake and does no work, as a hook's `caffeinate -w <cli pid>` does. */
	readonly keepsAwake: boolean;
};

/** An agent's processes, as the Running list shows them. */
export type RunningList = {
	/** What the agent started, one group per command: shells, dev servers, watchers and builds. */
	readonly started: readonly ProcessGroup[];
	/** The agent's CLI and its MCP servers, for the group at the bottom. */
	readonly own: readonly ProcessRow[];
};

// An MCP server's command names MCP, as `maxd mcp` and most servers do.
const MCP = /mcp|modelcontextprotocol/i;

// The shells a CLI runs each command in, and the programs that keep the
// machine awake: `caffeinate` on a Mac and `systemd-inhibit` on Linux.
// maxd leaves the same trees out of what an agent left running.
const SHELLS: ReadonlySet<string> = new Set(["sh", "bash", "zsh", "dash", "fish", "ksh"]);
const KEEPERS: ReadonlySet<string> = new Set(["caffeinate", "systemd-inhibit"]);

/** Whether `command` waits for `pid` to exit, as `tail --pid=<cli pid> -f /dev/null` does. */
const waitsOn = (command: string, pid: number): boolean => {
	const words = command.split(/\s+/);
	return words.some(
		(word, at) => word === `--pid=${pid}` || (word === "--pid" && words[at + 1] === `${pid}`)
	);
};

/**
 * Whether nothing in `all` does work: each process keeps the machine
 * awake, waits for the CLI to exit, or is a shell running one of those.
 */
const onlyKeepsAwake = (all: readonly AgentProcess[], cli: number | undefined): boolean => {
	const parents = new Set(all.map((process) => process.parent));
	return all.every(
		(process) =>
			KEEPERS.has(process.name) ||
			(cli !== undefined && waitsOn(process.command, cli)) ||
			(SHELLS.has(process.name.replace(/^-+/, "")) && parents.has(process.pid))
	);
};

const oldest = (a: AgentProcess, b: AgentProcess) => a.start - b.start || a.pid - b.pid;

const byPid = (processes: readonly AgentProcess[]) =>
	new Map(processes.map((process) => [process.pid, process]));

/**
 * Which processes are the agent's own: its CLI, an MCP server the CLI
 * started, and anything under one. A loop in the parents, which a reused
 * pid could make, counts as started.
 */
const ownPids = (processes: readonly AgentProcess[]): ReadonlySet<number> => {
	const pids = byPid(processes);
	const known = new Map<number, boolean>();
	const isOwn = (process: AgentProcess): boolean => {
		const seen = known.get(process.pid);
		if (seen !== undefined) return seen;
		known.set(process.pid, false);
		const parent = pids.get(process.parent);
		const own =
			process.cli ||
			(parent !== undefined && (parent.cli ? MCP.test(process.command) : isOwn(parent)));
		known.set(process.pid, own);
		return own;
	};
	return new Set(processes.filter(isOwn).map((process) => process.pid));
};

/**
 * Each process whose parent isn't in `processes` heads a group, with
 * everything under it, oldest first. `cli` is the agent's CLI's pid.
 */
const grouped = (processes: readonly AgentProcess[], cli: number | undefined): ProcessGroup[] => {
	const pids = byPid(processes);
	const known = new Map<number, number>();
	const headOf = (process: AgentProcess): number => {
		const seen = known.get(process.pid);
		if (seen !== undefined) return seen;
		known.set(process.pid, process.pid);
		const parent = pids.get(process.parent);
		const head = parent === undefined ? process.pid : headOf(parent);
		known.set(process.pid, head);
		return head;
	};
	const sorted = [...processes].sort(oldest);
	const under = new Map<number, AgentProcess[]>();
	for (const process of sorted) {
		const head = headOf(process);
		if (head !== process.pid) under.set(head, [...(under.get(head) ?? []), process]);
	}
	return sorted
		.filter((process) => headOf(process) === process.pid)
		.map((head) => {
			const below = under.get(head.pid) ?? [];
			const all = [head, ...below];
			return {
				head,
				under: below,
				ports: [...new Set(all.flatMap((process) => process.ports))].sort((a, b) => a - b),
				memory: all.reduce((sum, process) => sum + process.memory, 0),
				keepsAwake: onlyKeepsAwake(all, cli),
			};
		});
};

/** A group as rows: its head, then everything under it one step in. */
const rowsOf = (groups: readonly ProcessGroup[]): ProcessRow[] =>
	groups.flatMap(({ head, under }) => [
		{ process: head, child: false },
		...under.map((process) => ({ process, child: true })),
	]);

const rank = (group: ProcessGroup) => (group.keepsAwake ? 2 : group.ports.length > 0 ? 0 : 1);

/**
 * Splits an agent's processes into what it started, one group per command
 * with the ones on a port first and the ones that only keep the machine
 * awake last, and its own as rows.
 */
export const runningList = (processes: readonly AgentProcess[]): RunningList => {
	const own = ownPids(processes);
	const cli = processes.find((process) => process.cli)?.pid;
	const groupsOf = (owned: boolean) =>
		grouped(
			processes.filter((process) => own.has(process.pid) === owned),
			cli
		);
	return {
		started: groupsOf(false).sort((a, b) => rank(a) - rank(b)),
		own: rowsOf(groupsOf(true)),
	};
};

/**
 * The order to stop a group in: newest first, so each process gets its
 * signal before the one that started it and a shell can't leave its
 * server running when it goes.
 */
export const stopOrder = (group: ProcessGroup): readonly AgentProcess[] => [
	...[...group.under].reverse(),
	group.head,
];

/** The states an agent is in while a turn runs, waiting on you or a limit included. */
const IN_TURN: ReadonlySet<Activity["state"]> = new Set([
	"starting",
	"busy",
	"busy_in_subagents",
	"needs_input",
	"waiting_for_limit_reset",
]);

/**
 * Whether the agent's turn has ended, so what it started and still runs
 * outlived the turn. A sub-agent's processes run under its lead's holder,
 * so its lead's turn is the one that counts. An agent the view doesn't
 * have marks nothing.
 */
export const turnEnded = (view: ViewState, agent: AgentId): boolean => {
	let at = view.tables.agent.get(agent);
	const seen = new Set<AgentId>();
	while (at?.parent && !seen.has(at.id)) {
		seen.add(at.id);
		at = view.tables.agent.get(at.parent) ?? at;
	}
	return at !== undefined && !IN_TURN.has(at.activity.state);
};

/**
 * Each process's CPU use as a share of one core: since the read before
 * for one that was in it, else over its whole life so far.
 */
export const cpuShares = (
	processes: readonly AgentProcess[],
	before: readonly AgentProcess[]
): ReadonlyMap<string, number> => {
	const earlier = new Map(before.map((process) => [processKey(process), process]));
	return new Map(
		processes.map((process) => {
			const was = earlier.get(processKey(process));
			const ran = was ? process.run_ms - was.run_ms : 0;
			const share =
				was && ran > 0
					? Math.max(0, process.cpu_ms - was.cpu_ms) / ran
					: process.run_ms > 0
						? process.cpu_ms / process.run_ms
						: 0;
			return [processKey(process), share];
		})
	);
};

/** A share of one core as a percent, such as "0.4%" or "180%". */
export const formatCpu = (share: number): string => {
	const percent = Math.max(0, share) * 100;
	return formatNumber(percent / 100, {
		style: "percent",
		minimumFractionDigits: percent >= 100 ? 0 : 1,
		maximumFractionDigits: percent >= 100 ? 0 : 1,
	});
};

// Claude's Bash tool runs each command in a shell as `... && eval '<command>'
// ...`, with each quote in it written '\''.
const WRAPPED = /\beval '((?:[^']|'\\'')*)'/;

/**
 * A command line short enough for a row: the program without its folder,
 * or for a shell that Claude's Bash tool wrapped around a command, that
 * command. A process with no command line shows its name.
 */
export const shortCommand = (process: Pick<AgentProcess, "command" | "name">): string => {
	const command = process.command.trim();
	if (command === "") return process.name;
	const wrapped = WRAPPED.exec(command)?.[1];
	if (wrapped !== undefined) return wrapped.replaceAll("'\\''", "'").trim() || command;
	const [program = "", ...rest] = command.split(/\s+/);
	const base = nameOf(program);
	return [base, ...rest].join(" ");
};
