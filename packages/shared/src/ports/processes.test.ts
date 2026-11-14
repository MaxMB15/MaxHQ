// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Activity, AgentProcess } from "../bindings";
import { formatMemory } from "../fleet/memory";
import { formatDuration } from "../status/format";
import { agent, agentProcess, entities, MIN } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	cpuShares,
	formatCpu,
	processKey,
	type ProcessGroup,
	runningList,
	shortCommand,
	stopOrder,
	turnEnded,
} from "./processes";

const CLI = agentProcess({ pid: 100, parent: 90, start: 1, name: "claude", cli: true });

const pids = (rows: readonly { process: AgentProcess; child: boolean }[]) =>
	rows.map(({ process, child }) => (child ? `  ${process.pid}` : `${process.pid}`));

/** Each group's pids, its head first. */
const groups = (list: readonly ProcessGroup[]) =>
	list.map(({ head, under }) => [head.pid, ...under.map((process) => process.pid)]);

describe("the Running list", () => {
	it("leads with what the agent started and keeps its CLI and MCP servers apart", () => {
		const list = runningList([
			CLI,
			agentProcess({ pid: 101, start: 2, command: "maxd mcp --key-file /keys/a.key" }),
			agentProcess({ pid: 102, start: 3, command: "npx -y @modelcontextprotocol/server-x" }),
			agentProcess({ pid: 103, parent: 102, start: 4, name: "node", command: "node x" }),
			agentProcess({ pid: 110, start: 10, command: "/bin/zsh -c pnpm dev" }),
			agentProcess({ pid: 111, parent: 110, start: 11, command: "node vite", ports: [5173] }),
		]);
		expect(groups(list.started)).toEqual([[110, 111]]);
		expect(pids(list.own)).toEqual(["100", "  101", "  102", "  103"]);
	});

	it("counts a command that only mentions MCP under a shell as started", () => {
		const list = runningList([
			CLI,
			agentProcess({ pid: 110, start: 10, command: "/bin/zsh -c cargo test" }),
			agentProcess({ pid: 111, parent: 110, start: 11, command: "mcp-inspector" }),
		]);
		expect(groups(list.started)).toEqual([[110, 111]]);
		expect(pids(list.own)).toEqual(["100"]);
	});

	it("groups everything under a started process with it, oldest first", () => {
		const list = runningList([
			CLI,
			agentProcess({ pid: 120, start: 20 }),
			agentProcess({ pid: 112, parent: 111, start: 13, command: "esbuild" }),
			agentProcess({ pid: 111, parent: 110, start: 12, command: "node vite" }),
			agentProcess({ pid: 110, start: 11, command: "pnpm dev" }),
		]);
		expect(groups(list.started)).toEqual([[110, 111, 112], [120]]);
	});

	it("heads a row with a process whose parent left the tree", () => {
		// On macOS an orphan stays in the holder's group under launchd.
		const list = runningList([CLI, agentProcess({ pid: 130, parent: 1, start: 30 })]);
		expect(groups(list.started)).toEqual([[130]]);
	});

	it("ends a loop in the parents that a reused pid made", () => {
		const list = runningList([
			agentProcess({ pid: 140, parent: 141, start: 40 }),
			agentProcess({ pid: 141, parent: 140, start: 41 }),
		]);
		expect(groups(list.started)).toEqual([[140, 141]]);
		expect(list.own).toEqual([]);
	});

	it("puts the commands on a port first, and totals each one's ports and memory", () => {
		const list = runningList([
			CLI,
			agentProcess({ pid: 110, start: 10, command: "zsh -c cargo test", memory: 1 }),
			agentProcess({ pid: 111, parent: 110, start: 11, command: "rustc", memory: 2 }),
			agentProcess({ pid: 120, start: 20, command: "zsh -c pnpm dev", memory: 4 }),
			agentProcess({ pid: 121, parent: 120, start: 21, ports: [5173, 24678], memory: 8 }),
			agentProcess({ pid: 122, parent: 121, start: 22, ports: [5173], memory: 16 }),
			agentProcess({ pid: 130, start: 30, command: "sleep 600", memory: 32 }),
		]);
		expect(groups(list.started)).toEqual([[120, 121, 122], [110, 111], [130]]);
		const [server, build] = list.started;
		expect(server?.ports).toEqual([5173, 24678]);
		expect(server?.memory).toBe(28);
		expect(build?.ports).toEqual([]);
		expect(build?.memory).toBe(3);
	});

	it("puts what only keeps the machine awake last, and marks it", () => {
		const list = runningList([
			CLI,
			// A common hook's keeper, for the CLI's whole life.
			agentProcess({
				pid: 101,
				start: 2,
				command:
					'/bin/sh -c pgrep -f "caffeinate -di -w $PPID" >/dev/null 2>&1 || caffeinate -di -w $PPID',
			}),
			agentProcess({
				pid: 102,
				parent: 101,
				start: 3,
				name: "caffeinate",
				command: "caffeinate -di -w 100",
			}),
			// The CLI's own keeper for a turn.
			agentProcess({
				pid: 103,
				start: 4,
				name: "caffeinate",
				command: "caffeinate -i -t 300",
			}),
			// On Linux, a keeper that waits for the CLI to exit.
			agentProcess({
				pid: 104,
				start: 5,
				name: "systemd-inhibit",
				command: "systemd-inhibit tail --pid=100 -f /dev/null",
			}),
			agentProcess({
				pid: 105,
				parent: 104,
				start: 6,
				name: "tail",
				command: "tail --pid=100 -f /dev/null",
			}),
			// A keeper that runs a build is the build.
			agentProcess({
				pid: 110,
				start: 10,
				name: "zsh",
				command: "zsh -c caffeinate -i cargo build",
			}),
			agentProcess({
				pid: 111,
				parent: 110,
				start: 11,
				name: "caffeinate",
				command: "caffeinate -i cargo build",
			}),
			agentProcess({
				pid: 112,
				parent: 111,
				start: 12,
				name: "cargo",
				command: "cargo build",
			}),
			// A shell with nothing under it runs its own script.
			agentProcess({
				pid: 120,
				start: 20,
				name: "bash",
				command: "bash -c while true; do :; done",
			}),
			agentProcess({
				pid: 130,
				start: 30,
				name: "tail",
				command: "tail --pid=99 -f log.txt",
			}),
		]);
		expect(groups(list.started)).toEqual([
			[110, 111, 112],
			[120],
			[130],
			[101, 102],
			[103],
			[104, 105],
		]);
		expect(list.started.map((group) => group.keepsAwake)).toEqual([
			false,
			false,
			false,
			true,
			true,
			true,
		]);
		expect(pids(list.own)).toEqual(["100"]);
	});

	it("stops a command newest first, so each process goes before the one that started it", () => {
		const [group] = runningList([
			agentProcess({ pid: 110, start: 10 }),
			agentProcess({ pid: 111, parent: 110, start: 11 }),
			agentProcess({ pid: 112, parent: 111, start: 12 }),
			agentProcess({ pid: 113, parent: 110, start: 13 }),
		]).started;
		expect(group && stopOrder(group).map((process) => process.pid)).toEqual([
			113, 112, 111, 110,
		]);
	});

	it("names a process by its pid and start, so a reused pid is another", () => {
		expect(processKey({ pid: 5, start: 9 })).not.toBe(processKey({ pid: 5, start: 10 }));
	});
});

describe("after the turn", () => {
	const viewOf = (lead: Activity, sub: Activity = { state: "done_unread" }) =>
		applySnapshot({
			seq: 1,
			entities: entities({
				agents: [
					agent({ id: "lead", activity: lead }),
					agent({ id: "sub", parent: "lead", activity: sub }),
				],
			}),
		});

	it("marks nothing while a turn runs, waits on you or waits for a limit", () => {
		for (const activity of [
			{ state: "starting" },
			{ state: "busy" },
			{ state: "busy_in_subagents" },
			{ state: "needs_input", attention: "att-1" },
			{ state: "waiting_for_limit_reset", resets_at: null },
		] satisfies Activity[]) {
			expect(turnEnded(viewOf(activity), "lead")).toBe(false);
		}
	});

	it("marks what outlived the turn once the agent is done", () => {
		for (const activity of [
			{ state: "done_unread" },
			{ state: "idle" },
			{ state: "idle_cache_warm", warm_until: 0 },
		] satisfies Activity[]) {
			expect(turnEnded(viewOf(activity), "lead")).toBe(true);
		}
	});

	it("asks a sub-agent's lead, whose holder runs its processes", () => {
		expect(turnEnded(viewOf({ state: "busy_in_subagents" }), "sub")).toBe(false);
		expect(turnEnded(viewOf({ state: "idle" }, { state: "busy" }), "sub")).toBe(true);
	});

	it("marks nothing for an agent the view doesn't have", () => {
		expect(turnEnded(viewOf({ state: "idle" }), "gone")).toBe(false);
	});
});

describe("the numbers on a row", () => {
	it("takes CPU since the read before, or over the process's life on the first", () => {
		const first = agentProcess({ pid: 7, run_ms: 10_000, cpu_ms: 1_000 });
		const later = { ...first, run_ms: 12_000, cpu_ms: 2_000 };
		expect(cpuShares([first], []).get(processKey(first))).toBeCloseTo(0.1);
		expect(cpuShares([later], [first]).get(processKey(later))).toBeCloseTo(0.5);
		// A reused pid has nothing before it.
		const other = { ...later, start: 99 };
		expect(cpuShares([other], [first]).get(processKey(other))).toBeCloseTo(2 / 12);
		expect(cpuShares([{ ...first, run_ms: 0 }], []).get(processKey(first))).toBe(0);
	});

	it("shows CPU in tenths of a percent, and whole ones past one core", () => {
		expect(formatCpu(0)).toBe("0.0%");
		expect(formatCpu(0.004)).toBe("0.4%");
		expect(formatCpu(0.123)).toBe("12.3%");
		expect(formatCpu(1.8)).toBe("180%");
	});

	it("shows memory and how long it has run in short forms", () => {
		expect(formatMemory(512)).toBe("512 B");
		expect(formatMemory(84 * 1024 * 1024)).toBe("84.0 MB");
		expect(formatMemory(1.5 * 1024 * 1024 * 1024)).toBe("1.50 GB");
		expect(formatDuration(72_000)).toBe("1 min 12 s");
		expect(formatDuration(142 * MIN)).toBe("2 h 22 min");
	});
});

describe("a row's command", () => {
	it("drops the program's folder", () => {
		const process = { name: "node", command: "/opt/node/bin/node ./node_modules/.bin/vite" };
		expect(shortCommand(process)).toBe("node ./node_modules/.bin/vite");
	});

	it("shows the command Claude's Bash tool wrapped in a shell", () => {
		const command =
			"/bin/zsh -c -l source /home/dev/.claude/shell-snapshots/snapshot-zsh-1.sh && " +
			"eval 'pnpm dev --filter '\\''./apps/web'\\''' \\< /dev/null && pwd -P >| /tmp/claude-1-cwd";
		expect(shortCommand({ name: "zsh", command })).toBe("pnpm dev --filter './apps/web'");
	});

	it("falls back to the name when there's no command line", () => {
		expect(shortCommand({ name: "kworker", command: "  " })).toBe("kworker");
	});
});
