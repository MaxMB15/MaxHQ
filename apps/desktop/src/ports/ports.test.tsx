// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import type { Agent, Delta, Entity, ListeningPort, Workspace } from "@maxhq/shared/bindings";
import { FixedNow } from "@maxhq/shared/lib/clock";
import {
	agent as buildAgent,
	device,
	entities,
	link,
	network,
	NOW,
	project,
	workspace as buildWorkspace,
} from "@maxhq/shared/test/build";
import { remoteCalls } from "@maxhq/shared/test/remote";
import { applySnapshot, keyOf } from "@maxhq/shared/view/replica";

import { PortOffers } from "./offer";
import { closedForwards } from "@maxhq/shared/ports/select";
import { clearPorts } from "@maxhq/shared/ports/store";
import { WorkspacePorts } from "./workspace-ports";

afterEach(() => clearPorts());

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const REMOTE_WS: Workspace = buildWorkspace({
	id: "ws-maxhq",
	project: "proj-maxhq",
	device: "dev-devbox",
	location: { type: "container", instance: "ws-maxhq", path: "/home/agent/work/MaxHQ" },
});

const LOCAL_WS: Workspace = buildWorkspace({
	id: "ws-site",
	device: "dev-mac",
	location: { type: "folder", path: "/Users/someone/Code/Site" },
});

const remoteAgent = buildAgent({ id: "a1", workspace: REMOTE_WS.id, control: "held" });
const otherAgent = buildAgent({ id: "a2", workspace: REMOTE_WS.id, control: "held" });
const localAgent = buildAgent({ id: "a3", workspace: LOCAL_WS.id, control: "held" });

const port = (id: string, number: number, over: Partial<ListeningPort> = {}): Entity => ({
	entity: "listening_port",
	id,
	workspace: REMOTE_WS.id,
	agent: remoteAgent.id,
	port: number,
	bind: { type: "any" },
	process: { pid: 412, name: "vite", command: "pnpm dev" },
	url_hint: null,
	first_seen: NOW - 4000,
	gone_since: null,
	...over,
});

const forward = (id: string, portId: string, local: number): Entity => ({
	entity: "forward",
	id,
	port: portId,
	client: "client-1",
	local_port: local,
	opened_at: NOW,
});

const base = (): Entity[] => [
	...entities({
		agents: [remoteAgent, otherAgent, localAgent],
		devices: [
			device({ id: "dev-mac", name: "Mac", os: "macos" }),
			device({ id: "dev-devbox" }),
		],
		links: [
			link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
			link({ id: "devbox", device: "dev-devbox", network: "tailnet" }),
		],
		networks: [network()],
		projects: [project("proj-maxhq", "MaxHQ")],
		workspaces: [REMOTE_WS, LOCAL_WS],
	}),
];

/** A source over the base view plus `extra`, whose port calls are spies. */
const fakeSource = (extra: Entity[]) => {
	let emit: (delta: Delta) => void = () => {};
	let seq = 1;
	const source = {
		kind: "mock",
		snapshot: async () => ({ seq: 1, entities: [...base(), ...extra] }),
		subscribe: async (onDelta) => {
			emit = onDelta;
			return () => {};
		},
		onFocusAgent: async () => () => {},
		appInfo: async () => null,
		markSeen: async () => ({ status: "ok", data: null }),
		refreshUsage: async () => ({ status: "ok", data: null }),
		refreshLinks: async () => ({ status: "ok", data: null }),
		setShown: async () => {},
		openTimeline: async () => () => {},
		loadOlder: async () => ({ status: "ok", data: null }),
		openWorkflow: async () => () => {},
		nodeDetail: async () => ({ status: "ok", data: { lists: [] } }),
		startAgent: async () => DONE,
		sendInput: async () => DONE,
		cancelQueued: async () => DONE,
		interrupt: async () => DONE,
		setPermission: async () => DONE,
		setModel: async () => DONE,
		setEffort: async () => DONE,
		stopAgent: async () => DONE,
		setInbox: async (agents) => agents.map(() => DONE),
		continueHere: async () => DONE,
		takeOver: async () => DONE,
		answer: async () => DONE,
		stageAttachments: async () => [],
		stagePasted: async () => DONE,
		setDeviceSettings: async () => DONE,
		...remoteCalls(),
		openForward: vi.fn<DataSource["openForward"]>(async () => ({
			status: "ok",
			data: { forward: "fwd-1", local_port: 5173 },
		})),
		closeForward: vi.fn<DataSource["closeForward"]>(async () => {}),
		openLocalhost: vi.fn<DataSource["openLocalhost"]>(async () => {}),
	} satisfies DataSource;

	const change = (changes: Delta["changes"]) =>
		act(() => {
			seq += 1;
			emit({ seq, changes });
		});
	return {
		source,
		upsert: (values: Entity[]) => change(values.map((value) => ({ op: "upsert", value }))),
		remove: (values: Entity[]) =>
			change(values.map((value) => ({ op: "remove", key: keyOf(value) }))),
	};
};

const show = (agent: Agent, extra: Entity[] = []) => {
	const fake = fakeSource(extra);
	const workspace = agent.workspace === LOCAL_WS.id ? LOCAL_WS : REMOTE_WS;
	render(
		<ServicesProvider source={fake.source}>
			<FixedNow value={NOW}>
				<WorkspacePorts agent={agent} workspace={workspace} />
				<PortOffers agent={agent} />
			</FixedNow>
		</ServicesProvider>
	);
	return fake;
};

const card = (number: number) => screen.findByRole("region", { name: `Port ${number}` });

describe("port offers", () => {
	it("offers a forward when the agent's process starts listening, and opens it", async () => {
		const fake = show(remoteAgent);
		await screen.findByRole("button", { name: "Workspace ws-maxhq on devbox" });
		expect(screen.queryByRole("list", { name: "Ports" })).not.toBeInTheDocument();

		fake.upsert([port("p1", 5173)]);
		const offer = await card(5173);
		expect(offer).toHaveTextContent("MaxHQ on devbox is listening on port 5173.");
		expect(offer).toHaveTextContent("Forward to localhost:5173?");

		fireEvent.click(within(offer).getByRole("button", { name: "Forward" }));
		expect(fake.source.openForward).toHaveBeenCalledWith("p1");
		fake.upsert([forward("fwd-1", "p1", 5173)]);

		const forwarded = await card(5173);
		await waitFor(() => expect(forwarded).toHaveTextContent("Forwarded to localhost:5173"));
		expect(forwarded).toHaveTextContent("vite from MaxHQ on devbox");
		fireEvent.click(within(forwarded).getByRole("button", { name: "Open in browser" }));
		expect(fake.source.openLocalhost).toHaveBeenCalledWith(5173);
		expect(
			screen.getByRole("button", { name: "Workspace ws-maxhq on devbox, 1 forwarded port" })
		).toBeInTheDocument();
	});

	it("says when the port was busy here and the forward took the next one", async () => {
		show(remoteAgent, [port("p1", 5173), forward("fwd-1", "p1", 5174)]);
		const forwarded = await card(5173);
		expect(forwarded).toHaveTextContent("5173 is busy on this Mac, forwarded to 5174");
		fireEvent.click(within(forwarded).getByRole("button", { name: "Open in browser" }));
	});

	it("shows only the agent's own ports, and none of the other agent's", async () => {
		show(remoteAgent, [port("p1", 5173), port("p2", 8787, { agent: otherAgent.id })]);
		await card(5173);
		expect(screen.queryByRole("region", { name: "Port 8787" })).not.toBeInTheDocument();
	});

	it("says why a forward didn't open", async () => {
		const fake = show(remoteAgent, [port("p1", 5173)]);
		fake.source.openForward.mockResolvedValueOnce({
			status: "error",
			error: { kind: "not_connected", device: "devbox" },
		});
		fireEvent.click(within(await card(5173)).getByRole("button", { name: "Forward" }));
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't forward port 5173. devbox isn't connected."
		);
	});

	it("drops the offer on Dismiss and keeps the port in the workspace popover", async () => {
		show(remoteAgent, [port("p1", 5173)]);
		fireEvent.click(within(await card(5173)).getByRole("button", { name: "Dismiss" }));
		expect(screen.queryByRole("list", { name: "Ports" })).not.toBeInTheDocument();

		fireEvent.click(screen.getByRole("button", { name: "Workspace ws-maxhq on devbox" }));
		const popup = await screen.findByRole("dialog");
		const listening = within(popup).getByRole("region", { name: "Listening, not forwarded" });
		expect(listening).toHaveTextContent("5173");
		expect(within(listening).getByRole("button", { name: "Forward port 5173" })).toBeEnabled();
	});

	it("stops a forward and keeps the offer away after", async () => {
		const fake = show(remoteAgent, [port("p1", 5173), forward("fwd-1", "p1", 5173)]);
		const forwarded = await card(5173);
		fireEvent.click(within(forwarded).getByRole("button", { name: "Stop forwarding" }));
		expect(fake.source.closeForward).toHaveBeenCalledWith("fwd-1");
		fake.remove([forward("fwd-1", "p1", 5173)]);
		await waitFor(() =>
			expect(screen.queryByRole("list", { name: "Ports" })).not.toBeInTheDocument()
		);
	});

	it("goes when the port closes, and says when a forward stopped with it", async () => {
		const fake = show(remoteAgent, [
			port("p1", 5173),
			forward("fwd-1", "p1", 5174),
			port("p2", 9229, { process: { pid: 9, name: "node", command: "node --inspect" } }),
		]);
		await card(5173);
		await card(9229);

		// Both close. The one with no forward just goes.
		fake.upsert([port("p1", 5173, { gone_since: NOW }), port("p2", 9229, { gone_since: NOW })]);
		await waitFor(() =>
			expect(screen.queryByRole("region", { name: "Port 5173" })).not.toBeInTheDocument()
		);
		expect(screen.queryByRole("region", { name: "Port 9229" })).not.toBeInTheDocument();
		const notice = "Port 5173 closed on devbox. Forwarding to localhost:5174 stopped.";
		expect(screen.getByText(notice)).toBeInTheDocument();

		// The notice outlives the port and its forward, until dismissed.
		fake.remove([port("p1", 5173), forward("fwd-1", "p1", 5174), port("p2", 9229)]);
		expect(screen.getByText(notice)).toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Dismiss the notice about port 5173" }));
		expect(screen.queryByText(notice)).not.toBeInTheDocument();
	});

	it("asks again when the port opens again", async () => {
		const fake = show(remoteAgent, [port("p1", 5173), forward("fwd-1", "p1", 5173)]);
		await card(5173);
		fake.upsert([port("p1", 5173, { gone_since: NOW })]);
		fake.remove([port("p1", 5173), forward("fwd-1", "p1", 5173)]);
		await screen.findByText(/^Port 5173 closed on devbox/);

		fake.upsert([port("p3", 5173)]);
		const offer = await card(5173);
		expect(within(offer).getByRole("button", { name: "Forward" })).toBeInTheDocument();
		expect(screen.queryByText(/^Port 5173 closed on devbox/)).not.toBeInTheDocument();
	});

	it("offers no forward while the device can't be reached, and keeps the forwards it has", async () => {
		const offline = {
			entity: "device_link" as const,
			...link({
				id: "devbox",
				device: "dev-devbox",
				network: "tailnet",
				state: { state: "unreachable", reason: "timed out", last_seen: NOW - 60_000 },
			}),
		};
		const fake = show(remoteAgent, [
			offline,
			port("p1", 5173),
			port("p2", 8080),
			forward("fwd-2", "p2", 8080),
		]);
		const forwarded = await card(8080);
		expect(forwarded).toHaveTextContent(
			"devbox can't be reached. The forward picks up again once it's back."
		);
		expect(screen.queryByRole("region", { name: "Port 5173" })).not.toBeInTheDocument();

		fake.upsert([{ ...offline, state: { state: "connected", since: NOW } }]);
		const offer = await card(5173);
		expect(within(offer).getByRole("button", { name: "Forward" })).toBeInTheDocument();
		expect(await card(8080)).toHaveTextContent("vite from MaxHQ on devbox");
	});

	it("offers no forward on a device that doesn't forward ports", async () => {
		const fake = show(remoteAgent, [
			{
				entity: "device_link",
				...link({
					id: "devbox",
					device: "dev-devbox",
					network: "tailnet",
					capabilities: { transfers: true, tunnels: false, workspaces: true },
				}),
			},
			port("p1", 5173),
			port("p2", 6379, { agent: null }),
		]);
		fireEvent.click(
			await screen.findByRole("button", { name: "Workspace ws-maxhq on devbox" })
		);
		const popup = await screen.findByRole("dialog");
		expect(popup).toHaveTextContent("devbox doesn't forward ports.");
		const listening = within(popup).getByRole("region", { name: "Listening, not forwarded" });
		expect(listening).toHaveTextContent("5173");
		expect(within(listening).queryByRole("button")).not.toBeInTheDocument();
		const others = within(popup).getByRole("region", { name: "Other ports" });
		expect(within(others).queryByRole("button")).not.toBeInTheDocument();
		expect(screen.queryByRole("list", { name: "Ports" })).not.toBeInTheDocument();
		expect(fake.source.openForward).not.toHaveBeenCalled();
	});

	it("opens a Mac agent's port on localhost, with no forward", async () => {
		const fake = show(localAgent, [
			port("p1", 3000, { workspace: LOCAL_WS.id, agent: localAgent.id }),
		]);
		const offer = await card(3000);
		expect(offer).toHaveTextContent("Site is listening on port 3000.");
		expect(within(offer).queryByRole("button", { name: "Forward" })).not.toBeInTheDocument();
		fireEvent.click(within(offer).getByRole("button", { name: "Open localhost:3000" }));
		expect(fake.source.openLocalhost).toHaveBeenCalledWith(3000);
		expect(fake.source.openForward).not.toHaveBeenCalled();
	});
});

describe("workspace ports popover", () => {
	it("lists forwarded ports, the agents' other ports and the rest", async () => {
		const fake = show(remoteAgent, [
			port("p1", 5173),
			forward("fwd-1", "p1", 5174),
			port("p2", 9229, {
				agent: otherAgent.id,
				process: { pid: 9, name: "node", command: "node --inspect" },
			}),
			port("p3", 6379, {
				agent: null,
				process: { pid: 3, name: "redis-server", command: "redis-server *:6379" },
			}),
			port("p4", 4000, { gone_since: NOW - 2000 }),
		]);
		fireEvent.click(
			await screen.findByRole("button", {
				name: "Workspace ws-maxhq on devbox, 1 forwarded port",
			})
		);
		const popup = await screen.findByRole("dialog");
		expect(popup).toHaveTextContent("Incus container");
		expect(popup).toHaveTextContent("On devbox, over Tailscale. 2 agents work here.");
		expect(popup).not.toHaveTextContent("4000");

		const forwarded = within(popup).getByRole("region", { name: "Forwarded ports" });
		expect(forwarded).toHaveTextContent("5173");
		expect(forwarded).toHaveTextContent("5173 is busy on this Mac, forwarded to 5174");
		fireEvent.click(
			within(forwarded).getByRole("button", { name: "Open localhost:5174 in the browser" })
		);
		expect(fake.source.openLocalhost).toHaveBeenCalledWith(5174);
		fireEvent.click(within(forwarded).getByRole("button", { name: "Stop forwarding 5173" }));
		expect(fake.source.closeForward).toHaveBeenCalledWith("fwd-1");

		const listening = within(popup).getByRole("region", { name: "Listening, not forwarded" });
		fireEvent.click(within(listening).getByRole("button", { name: "Forward port 9229" }));
		expect(fake.source.openForward).toHaveBeenCalledWith("p2");

		const others = within(popup).getByRole("region", { name: "Other ports" });
		expect(others).toHaveTextContent("redis-server");
		expect(within(others).getByRole("button", { name: "Forward port 6379" })).toBeEnabled();
	});

	it("offers Open for each port on this Mac", async () => {
		const fake = show(localAgent, [
			port("p1", 3000, { workspace: LOCAL_WS.id, agent: localAgent.id }),
		]);
		fireEvent.click(await screen.findByRole("button", { name: "Workspace Site on Mac" }));
		const popup = await screen.findByRole("dialog");
		expect(popup).toHaveTextContent("On this Mac. 1 agent works here.");
		const listening = within(popup).getByRole("region", { name: "Listening" });
		fireEvent.click(
			within(listening).getByRole("button", { name: "Open localhost:3000 in the browser" })
		);
		expect(fake.source.openLocalhost).toHaveBeenCalledWith(3000);
	});

	it("offers no forward while the device can't be reached", async () => {
		const offline = {
			entity: "device_link" as const,
			...link({
				id: "devbox",
				device: "dev-devbox",
				network: "tailnet",
				state: { state: "unreachable", reason: "timed out", last_seen: NOW - 60_000 },
			}),
		};
		const fake = show(remoteAgent, [offline, port("p1", 5173)]);
		fireEvent.click(
			await screen.findByRole("button", { name: "Workspace ws-maxhq on devbox" })
		);
		const popup = await screen.findByRole("dialog");
		expect(popup).toHaveTextContent(
			"devbox can't be reached right now. Forwards pick up again once it's back."
		);
		const listening = within(popup).getByRole("region", { name: "Listening, not forwarded" });
		expect(listening).toHaveTextContent("5173");
		expect(
			within(listening).queryByRole("button", { name: "Forward port 5173" })
		).not.toBeInTheDocument();

		fake.upsert([{ ...offline, state: { state: "connected", since: NOW } }]);
		expect(
			await within(listening).findByRole("button", { name: "Forward port 5173" })
		).toBeEnabled();
		expect(popup).not.toHaveTextContent("can't be reached");
	});
});

describe("closed forwards", () => {
	const view = (extra: Entity[]) => applySnapshot({ seq: 1, entities: [...base(), ...extra] });

	it("counts a forward whose port closes, once", () => {
		const open = view([port("p1", 5173), forward("fwd-1", "p1", 5174)]);
		const lingering = view([
			port("p1", 5173, { gone_since: NOW }),
			forward("fwd-1", "p1", 5174),
		]);
		const gone = view([]);
		expect(closedForwards(open, lingering)).toEqual([
			{ agent: "a1", port: 5173, localPort: 5174, device: "dev-devbox" },
		]);
		expect(closedForwards(lingering, gone)).toEqual([]);
		expect(closedForwards(open, gone)).toHaveLength(1);
	});

	it("leaves out a forward stopped while its port still listens", () => {
		const open = view([port("p1", 5173), forward("fwd-1", "p1", 5173)]);
		expect(closedForwards(open, view([port("p1", 5173)]))).toEqual([]);
	});
});
