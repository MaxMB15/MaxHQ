// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AgentProcess } from "../bindings";
import { runningList } from "../ports/processes";
import { agent, NOW } from "../test/build";
import { mockProcesses } from "./mock-processes";

const AGENTS = [
	agent({ id: "agent-gtfs", pid: 812, started_at: NOW - 300_000 }),
	agent({ id: "agent-sidebar-sort", pid: 50_133 }),
	agent({ id: "agent-w1", parent: "agent-sidebar-sort", pid: null }),
	agent({ id: "agent-gone", pid: null }),
];

const calls = () =>
	mockProcesses(
		() => NOW,
		(id) => AGENTS.find((a) => a.id === id)
	);

const list = async (source: ReturnType<typeof calls>, id: string): Promise<AgentProcess[]> => {
	const result = await source.listProcesses(id);
	if (result.status !== "ok" || result.data.status !== "listed") throw new Error("Not listed");
	return result.data.processes;
};

const refused = (reason: string) => ({ status: "ok", data: { status: "refused", reason } });

describe("the sample processes", () => {
	it("runs the transit map's dev server and a commit under the GTFS agent", async () => {
		const { started, own } = runningList(await list(calls(), "agent-gtfs"));
		const [server, commit] = started;
		expect(started).toHaveLength(2);
		expect(server && [server.head, ...server.under].map((p) => p.command)).toEqual([
			"/bin/bash -c pnpm dev",
			"/usr/local/bin/node /usr/local/bin/pnpm dev",
			"vite --host",
		]);
		expect(server?.ports).toEqual([5173]);
		expect(commit?.head.name).toBe("zsh");
		expect(commit?.under).toHaveLength(6);
		expect(own.map((row) => row.process.name)).toEqual(["claude", "maxd"]);
	});

	it("lists a sub-agent's lead, and nothing for an agent that isn't running", async () => {
		const source = calls();
		expect(await list(source, "agent-w1")).toEqual(await list(source, "agent-sidebar-sort"));
		expect(await list(source, "agent-gone")).toEqual([]);
		expect(await source.listProcesses("agent-nope")).toEqual({
			status: "error",
			error: { kind: "not_found", what: "agent agent-nope" },
		});
	});

	it("stops a process, and holds the watcher's worker on until Kill", async () => {
		const source = calls();
		const [shell, worker] = (await list(source, "agent-sidebar-sort")).filter(
			(p) => !p.cli && p.pid !== 50_134
		);
		if (!shell || !worker) throw new Error("No watcher");

		expect(await source.signalProcess("agent-w1", worker.pid, worker.start, "term")).toEqual({
			status: "ok",
			data: { status: "done" },
		});
		expect((await list(source, "agent-sidebar-sort")).map((p) => p.pid)).toContain(worker.pid);
		await source.signalProcess("agent-sidebar-sort", worker.pid, worker.start, "kill");
		await source.signalProcess("agent-sidebar-sort", shell.pid, shell.start, "term");
		expect((await list(source, "agent-sidebar-sort")).map((p) => p.pid)).toEqual([
			50_133, 50_134,
		]);
	});

	it("refuses the CLI, a process that's gone and a reused pid", async () => {
		const source = calls();
		const [cli, , shell] = await list(source, "agent-gtfs");
		if (!cli || !shell) throw new Error("No processes");
		expect(await source.signalProcess("agent-gtfs", cli.pid, cli.start, "term")).toEqual(
			refused(
				"Couldn't stop that process: that's the agent itself, so stop the agent instead"
			)
		);
		expect(
			await source.signalProcess("agent-gtfs", shell.pid, shell.start + 1, "term")
		).toEqual(
			refused(
				"Couldn't stop that process: that process has exited, and its pid now names another one"
			)
		);
		expect(await source.signalProcess("agent-gtfs", 9, 9, "term")).toEqual(
			refused(
				"Couldn't stop that process: that process isn't running in the agent's tree any more"
			)
		);
		expect(await source.signalProcess("agent-gone", 9, 9, "term")).toEqual(
			refused("That agent isn't running, so it has no processes to stop.")
		);
	});
});
