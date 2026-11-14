// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import type { Activity, Agent, AgentProcess, Entity, Workspace } from "@maxhq/shared/bindings";
import { mockSource } from "@maxhq/shared/data/mock-source";
import type { DataSource, ProcessesResult } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { KILL_AFTER_MS, PROCESS_REFRESH_MS } from "@maxhq/shared/ports/processes";
import {
	agent as buildAgent,
	agentProcess,
	device,
	entities,
	link,
	network,
	NOW,
	project,
	workspace as buildWorkspace,
} from "@maxhq/shared/test/build";

import { WorkspacePorts } from "./workspace-ports";

afterEach(() => vi.useRealTimers());

const DONE = { status: "ok", data: { status: "done" } } as const;

const LOCAL_WS: Workspace = buildWorkspace({
	id: "ws-site",
	device: "dev-mac",
	location: { type: "folder", path: "/Users/someone/Code/Site" },
});

const REMOTE_WS: Workspace = buildWorkspace({
	id: "ws-maxhq",
	project: "proj-maxhq",
	device: "dev-devbox",
	location: { type: "container", instance: "ws-maxhq", path: "/home/agent/work/MaxHQ" },
});

const PROCESSES: AgentProcess[] = [
	agentProcess({ pid: 100, parent: 90, name: "claude", command: "claude --verbose", cli: true }),
	agentProcess({ pid: 101, name: "maxd", command: "maxd mcp" }),
	agentProcess({
		pid: 110,
		name: "zsh",
		command: "/bin/zsh -c pnpm dev",
		run_ms: 72_000,
		cpu_ms: 7200,
	}),
	agentProcess({
		pid: 111,
		parent: 110,
		name: "node",
		command: "/opt/node/bin/node vite --host",
		memory: 84 * 1024 * 1024,
		ports: [5173],
	}),
];

const listed = (processes: AgentProcess[]): ProcessesResult => ({
	status: "ok",
	data: { status: "listed", processes },
});

/** The popover for an agent in `workspace`, over a source whose process calls are spies. */
const show = (
	over: Partial<Agent> = {},
	{ workspace = LOCAL_WS, extra = [] }: { workspace?: Workspace; extra?: Entity[] } = {}
) => {
	const agent = buildAgent({ id: "a1", workspace: workspace.id, control: "held", ...over });
	const source = {
		...mockSource({ tick: false }),
		snapshot: async () => ({
			seq: 1,
			entities: [
				...entities({
					agents: [agent],
					devices: [
						device({ id: "dev-mac", name: "Mac", os: "macos" }),
						device({ id: "dev-devbox" }),
					],
					links: [
						link({
							id: "mac",
							name: "Mac",
							device: "dev-mac",
							route: { type: "local" },
						}),
						link({ id: "devbox", device: "dev-devbox", network: "tailnet" }),
					],
					networks: [network()],
					projects: [project("proj-maxhq", "MaxHQ")],
					workspaces: [LOCAL_WS, REMOTE_WS],
				}),
				...extra,
			],
		}),
		subscribe: async () => () => {},
		listProcesses: vi.fn<DataSource["listProcesses"]>(async () => listed(PROCESSES)),
		signalProcess: vi.fn<DataSource["signalProcess"]>(async () => DONE),
	} satisfies DataSource;
	render(
		<ServicesProvider source={source}>
			<FixedNow value={NOW}>
				<WorkspacePorts agent={agent} workspace={workspace} />
			</FixedNow>
		</ServicesProvider>
	);
	return source;
};

const open = async () => {
	fireEvent.click(await screen.findByRole("button", { name: /^Workspace / }));
	return screen.findByRole("dialog");
};

const running = async () => screen.findByRole("region", { name: "Running" });

const unfold = (name: string) => {
	const chevron = screen.getByRole("button", { name: `The 2 processes of ${name}` });
	fireEvent.click(chevron);
	expect(chevron).toHaveAttribute("aria-expanded", "true");
};

const rowOf = (text: string) => {
	const row = screen.getByText(text).closest("li");
	if (!row) throw new Error(`No row for ${text}`);
	return row;
};

describe("the Running section", () => {
	it("reads nothing until the popover opens, then lists what the agent started below the ports", async () => {
		const source = show(
			{ activity: { state: "busy" } },
			{
				extra: [
					{
						entity: "listening_port",
						id: "p1",
						workspace: LOCAL_WS.id,
						agent: "a1",
						port: 5173,
						bind: { type: "any" },
						process: { pid: 111, name: "node", command: "vite --host" },
						url_hint: null,
						first_seen: NOW,
						gone_since: null,
					},
				],
			}
		);
		await screen.findByRole("button", { name: /^Workspace / });
		expect(source.listProcesses).not.toHaveBeenCalled();

		const popup = await open();
		const section = await running();
		expect(source.listProcesses).toHaveBeenCalledWith("a1");
		const listening = within(popup).getByRole("region", { name: "Listening" });
		expect(listening.compareDocumentPosition(section)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);

		// The command is one row, with the totals of what runs under it.
		const shell = rowOf("zsh -c pnpm dev");
		expect(shell).toHaveTextContent("1 min 12 s");
		expect(shell).toHaveTextContent("10.0% CPU");
		expect(shell).toHaveTextContent("85.0 MB");
		expect(shell).toHaveTextContent("2 processes");
		expect(within(shell).getByTitle("/bin/zsh -c pnpm dev")).toBeInTheDocument();
		expect(within(shell).getByTitle("Listening on port 5173")).toHaveTextContent("5173");
		expect(screen.queryByText("node vite --host")).not.toBeInTheDocument();

		unfold("zsh -c pnpm dev");
		const server = rowOf("node vite --host");
		expect(server).toHaveTextContent("84.0 MB");
		expect(within(server).getByTitle("Listening on port 5173")).toHaveTextContent("5173");
		expect(section).not.toHaveTextContent("after the turn");
	});

	it("keeps the agent's CLI and MCP servers in a collapsed group, with no Stop", async () => {
		show({ activity: { state: "busy" } });
		await open();
		const section = await running();
		expect(section).not.toHaveTextContent("claude --verbose");
		const group = within(section).getByRole("button", { name: "Agent" });
		expect(group).toHaveAttribute("aria-expanded", "false");

		fireEvent.click(group);
		expect(group).toHaveAttribute("aria-expanded", "true");
		const own = within(section).getByRole("list", { name: "Agent" });
		expect(own).toHaveTextContent("claude --verbose");
		expect(own).toHaveTextContent("maxd mcp");
		expect(own).toHaveTextContent("These stop with the agent.");
		expect(within(own).queryByRole("button")).not.toBeInTheDocument();
	});

	it("marks what still runs after the agent's turn ended", async () => {
		show({ activity: { state: "idle" } });
		await open();
		const section = await running();
		await waitFor(() => expect(rowOf("zsh -c pnpm dev")).toHaveTextContent("after the turn"));
		unfold("zsh -c pnpm dev");
		expect(rowOf("node vite --host")).not.toHaveTextContent("after the turn");
		expect(within(section).getAllByText("after the turn")).toHaveLength(1);
	});

	it("puts what only keeps the Mac awake last, marked so, with its Stop", async () => {
		const source = show({ activity: { state: "idle" } });
		source.listProcesses.mockImplementation(async () =>
			listed([
				...PROCESSES,
				agentProcess({
					pid: 105,
					name: "sh",
					command: "/bin/sh -c caffeinate -di -w $PPID",
				}),
				agentProcess({
					pid: 106,
					parent: 105,
					name: "caffeinate",
					command: "caffeinate -di -w 100",
				}),
			])
		);
		await open();
		const section = await running();
		const keeper = await waitFor(() => rowOf("sh -c caffeinate -di -w $PPID"));
		expect(keeper).toHaveTextContent("keeps awake");
		expect(keeper).not.toHaveTextContent("after the turn");
		expect(
			within(keeper).getByTitle("It only keeps the computer awake and does no work.")
		).toBeInTheDocument();
		expect(within(keeper).getByRole("button", { name: /^Stop / })).toBeInTheDocument();
		expect(rowOf("zsh -c pnpm dev")).toHaveTextContent("after the turn");
		expect(rowOf("zsh -c pnpm dev").compareDocumentPosition(keeper)).toBe(
			Node.DOCUMENT_POSITION_FOLLOWING
		);
		expect(within(section).getAllByText("keeps awake")).toHaveLength(1);
	});

	it("stops a command with everything under it, newest first, and offers Kill once it outlasts Stop", async () => {
		vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"], shouldAdvanceTime: true });
		const source = show();
		await open();
		await running();

		fireEvent.click(screen.getByRole("button", { name: "Stop zsh -c pnpm dev" }));
		const stop = screen.getByRole("button", { name: "Stop zsh -c pnpm dev" });
		expect(stop).toBeDisabled();
		expect(stop).toHaveTextContent("Stopping");
		await waitFor(() => expect(source.signalProcess).toHaveBeenCalledTimes(2));
		expect(source.signalProcess.mock.calls).toEqual([
			["a1", 111, 111, "term"],
			["a1", 110, 110, "term"],
		]);

		await act(() => vi.advanceTimersByTimeAsync(KILL_AFTER_MS));
		const kill = await screen.findByRole("button", { name: "Kill zsh -c pnpm dev" });

		source.listProcesses.mockResolvedValue(listed(PROCESSES.filter((p) => p.pid < 110)));
		fireEvent.click(kill);
		await waitFor(() => expect(source.signalProcess).toHaveBeenCalledTimes(4));
		expect(source.signalProcess).toHaveBeenLastCalledWith("a1", 110, 110, "kill");
		await waitFor(() => expect(screen.queryByText("zsh -c pnpm dev")).not.toBeInTheDocument());
	});

	it("stops one process under a command on its own", async () => {
		const source = show();
		await open();
		await running();
		unfold("zsh -c pnpm dev");
		fireEvent.click(screen.getByRole("button", { name: "Stop node vite --host" }));
		await waitFor(() => expect(source.signalProcess).toHaveBeenCalledTimes(1));
		expect(source.signalProcess).toHaveBeenCalledWith("a1", 111, 111, "term");
		expect(screen.getByRole("button", { name: "Stop zsh -c pnpm dev" })).toBeEnabled();
	});

	it("says why the device wouldn't stop it", async () => {
		const source = show();
		source.signalProcess.mockResolvedValue({
			status: "ok",
			data: {
				status: "refused",
				reason: "Couldn't stop that process: that process has already exited",
			},
		});
		await open();
		await running();
		fireEvent.click(screen.getByRole("button", { name: "Stop zsh -c pnpm dev" }));
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't stop that process: that process has already exited"
		);
		expect(screen.getByRole("button", { name: "Stop zsh -c pnpm dev" })).toBeEnabled();
	});

	it("reads again every few seconds while open", async () => {
		vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"], shouldAdvanceTime: true });
		const source = show();
		await open();
		await running();
		expect(source.listProcesses).toHaveBeenCalledTimes(1);
		await act(() => vi.advanceTimersByTimeAsync(PROCESS_REFRESH_MS));
		expect(source.listProcesses).toHaveBeenCalledTimes(2);
	});

	it("says when the device's maxd is too old, and asks no more", async () => {
		vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"], shouldAdvanceTime: true });
		const source = show();
		source.listProcesses.mockResolvedValue({ status: "ok", data: { status: "too_old" } });
		await open();
		expect(await running()).toHaveTextContent(
			"This device's maxd is too old to list processes."
		);
		await act(() => vi.advanceTimersByTimeAsync(3 * PROCESS_REFRESH_MS));
		expect(source.listProcesses).toHaveBeenCalledTimes(1);
		expect(screen.getByText("Nothing listens on a port here yet.")).toBeInTheDocument();
	});

	it("shows nothing new when the device can't be reached", async () => {
		const unreachable: Entity = {
			entity: "device_link",
			...link({
				id: "devbox",
				device: "dev-devbox",
				network: "tailnet",
				state: { state: "unreachable", reason: "timed out", last_seen: NOW - 60_000 },
			}),
		};
		const source = show({}, { workspace: REMOTE_WS, extra: [unreachable] });
		const popup = await open();
		expect(popup).toHaveTextContent("Nothing listens on a port here yet.");
		expect(within(popup).queryByRole("region", { name: "Running" })).not.toBeInTheDocument();
		expect(source.listProcesses).not.toHaveBeenCalled();
	});

	it("shows nothing when the list can't be read", async () => {
		const source = show();
		source.listProcesses.mockResolvedValue({
			status: "error",
			error: { kind: "not_connected", device: "Mac" },
		});
		const popup = await open();
		await waitFor(() => expect(source.listProcesses).toHaveBeenCalled());
		expect(within(popup).queryByRole("region", { name: "Running" })).not.toBeInTheDocument();
	});

	it("says when nothing the agent started runs, and keeps the group", async () => {
		const source = show();
		source.listProcesses.mockResolvedValue(listed(PROCESSES.filter((p) => p.pid < 110)));
		await open();
		const section = await running();
		expect(section).toHaveTextContent("Nothing the agent started is running.");
		expect(within(section).getByRole("button", { name: "Agent" })).toBeInTheDocument();
	});
});

describe("an agent with no processes", () => {
	it("leaves the section out", async () => {
		const source = show({ activity: { state: "idle" } satisfies Activity, pid: null });
		source.listProcesses.mockResolvedValue(listed([]));
		const popup = await open();
		await waitFor(() => expect(source.listProcesses).toHaveBeenCalled());
		expect(within(popup).queryByRole("region", { name: "Running" })).not.toBeInTheDocument();
	});
});
