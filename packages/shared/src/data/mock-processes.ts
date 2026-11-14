// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The processes under the sample agents, for the Running list. Each
// running agent has its CLI, and a Claude agent has `maxd mcp` under it.
// The GTFS agent runs the transit map's dev server, whose port is in the
// sample view, and a commit whose hook is building and testing, the busy
// list a short command makes. The sidebar sort agent left a test watcher running
// after its turn. Stop ends a process, except the watcher's worker, which
// holds on until Kill. They live in memory.

import type { Agent, AgentId, AgentProcess, Timestamp, Vendor } from "../bindings";
import type { ActionResult, DataSource } from "./source";

const MIN = 60_000;
const MB = 1024 * 1024;

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

/** A sample process, using `cpu` of a core. */
type Seed = Omit<AgentProcess, "start" | "run_ms" | "cpu_ms" | "cli"> & { readonly cpu: number };

/** A process an agent started `ago` ms before the sample view was made. */
type Started = Seed & { readonly ago: number };

// What the sample agents started, under their CLI's pid in the sample view.
const STARTED: Partial<Record<AgentId, readonly Started[]>> = {
	"agent-gtfs": [
		{
			pid: 2201,
			parent: 812,
			ago: 4 * MIN + 3000,
			cpu: 0,
			memory: 3 * MB,
			name: "bash",
			command: "/bin/bash -c pnpm dev",
			ports: [],
		},
		{
			pid: 2207,
			parent: 2201,
			ago: 4 * MIN + 2000,
			cpu: 0.004,
			memory: 58 * MB,
			name: "node",
			command: "/usr/local/bin/node /usr/local/bin/pnpm dev",
			ports: [],
		},
		{
			pid: 2214,
			parent: 2207,
			ago: 4 * MIN,
			cpu: 0.03,
			memory: 142 * MB,
			name: "node",
			command: "vite --host",
			ports: [5173],
		},
		{
			pid: 2301,
			parent: 812,
			ago: 9000,
			cpu: 0,
			memory: 2 * MB,
			name: "zsh",
			command: "/bin/zsh -c git commit -q -F - <<'EOF'",
			ports: [],
		},
		{
			pid: 2302,
			parent: 2301,
			ago: 9000,
			cpu: 0,
			memory: 6 * MB,
			name: "git",
			command: "git commit -q -F -",
			ports: [],
		},
		{
			pid: 2303,
			parent: 2302,
			ago: 9000,
			cpu: 0,
			memory: 2 * MB,
			name: "sh",
			command: "sh .githooks/pre-commit",
			ports: [],
		},
		{
			pid: 2304,
			parent: 2303,
			ago: 7000,
			cpu: 0,
			memory: 55 * MB,
			name: "node",
			command: "node /usr/local/bin/pnpm test",
			ports: [],
		},
		{
			pid: 2305,
			parent: 2304,
			ago: 7000,
			cpu: 0.022,
			memory: 172 * MB,
			name: "cargo",
			command: "cargo test --workspace",
			ports: [],
		},
		{
			pid: 2306,
			parent: 2305,
			ago: 3000,
			cpu: 0.974,
			memory: 754 * MB,
			name: "rustc",
			command: "rustc --crate-name objc2_foundation --edition=2021",
			ports: [],
		},
		{
			pid: 2307,
			parent: 2305,
			ago: 3000,
			cpu: 0.895,
			memory: 972 * MB,
			name: "rustc",
			command: "rustc --crate-name maxhq_protocol --edition=2021",
			ports: [],
		},
	],
	"agent-sidebar-sort": [
		{
			pid: 50_190,
			parent: 50_133,
			ago: 31 * MIN,
			cpu: 0,
			memory: 2 * MB,
			name: "zsh",
			command: "/bin/zsh -c pnpm vitest --watch",
			ports: [],
		},
		{
			pid: 50_194,
			parent: 50_190,
			ago: 31 * MIN,
			cpu: 0.012,
			memory: 212 * MB,
			name: "node",
			command: "node ./node_modules/.bin/vitest --watch",
			ports: [],
		},
	],
};

// The watcher's worker holds on after Stop, so the list offers Kill.
const HOLDS_ON = new Set([50_194]);

const CLI: Record<Vendor, Pick<AgentProcess, "name" | "command">> = {
	claude: { name: "claude", command: "claude --output-format stream-json --verbose" },
	codex: { name: "codex", command: "codex app-server" },
	cursor: { name: "cursor-agent", command: "cursor-agent acp" },
	acp: { name: "agent", command: "agent acp" },
};

const NOT_RUNNING = "That agent isn't running, so it has no processes to stop.";

/** The Running list's calls over the sample agents. */
export const mockProcesses = (
	now: () => Timestamp,
	agentOf: (agent: AgentId) => Agent | undefined
): Pick<DataSource, "listProcesses" | "signalProcess"> => {
	const made = now();
	const gone = new Set<string>();

	// A sample sub-agent's processes run under its lead, one step up.
	const holderOf = (agent: Agent) => (agent.parent ? agentOf(agent.parent) : agent) ?? agent;

	const treeOf = (agent: Agent): AgentProcess[] => {
		if (agent.pid === null) return [];
		const at = now();
		const process = ({ cpu, ...seed }: Seed, since: Timestamp, cli = false): AgentProcess => {
			const run_ms = Math.max(0, at - since);
			return { ...seed, start: since * 1000, run_ms, cpu_ms: Math.round(run_ms * cpu), cli };
		};
		const own = { parent: agent.pid, ports: [] };
		return [
			process(
				{
					...own,
					pid: agent.pid,
					parent: 1,
					cpu: 0.02,
					memory: 310 * MB,
					...CLI[agent.vendor],
				},
				agent.started_at,
				true
			),
			...(agent.vendor === "claude"
				? [
						process(
							{
								...own,
								pid: agent.pid + 1,
								cpu: 0,
								memory: 18 * MB,
								name: "maxd",
								command: "maxd mcp",
							},
							agent.started_at + 2000
						),
					]
				: []),
			...(STARTED[agent.id] ?? []).map(({ ago, ...seed }) => process(seed, made - ago)),
		].filter((p) => !gone.has(`${agent.id}:${p.pid}`));
	};

	return {
		listProcesses: async (id) => {
			const agent = agentOf(id);
			if (!agent)
				return { status: "error", error: { kind: "not_found", what: `agent ${id}` } };
			return { status: "ok", data: { status: "listed", processes: treeOf(holderOf(agent)) } };
		},
		signalProcess: async (id, pid, start, signal) => {
			const agent = agentOf(id);
			if (!agent)
				return { status: "error", error: { kind: "not_found", what: `agent ${id}` } };
			const holder = holderOf(agent);
			if (holder.pid === null) return refused(NOT_RUNNING);
			const found = treeOf(holder).find((p) => p.pid === pid);
			const why = !found
				? "that process isn't running in the agent's tree any more"
				: found.start !== start
					? "that process has exited, and its pid now names another one"
					: found.cli
						? "that's the agent itself, so stop the agent instead"
						: null;
			if (why) return refused(`Couldn't stop that process: ${why}`);
			if (signal === "kill" || !HOLDS_ON.has(pid)) gone.add(`${holder.id}:${pid}`);
			return DONE;
		},
	};
};
