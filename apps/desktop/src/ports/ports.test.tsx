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

import { PortPromptsSection } from "@/settings/port-prompts-settings";
import { PortOffers } from "./offer";
import { PORT_PROMPTS_KEY } from "./prompts-prefs";
import { closedForwards } from "@maxhq/shared/ports/select";
import { clearPorts } from "@maxhq/shared/ports/store";
import { WorkspacePorts } from "./workspace-ports";

afterEach(() => {
	clearPorts();
	localStorage.clear();
});

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

const show = (agent: Agent, extra: Entity[] = [], { settings = false } = {}) => {
	const fake = fakeSource(extra);
	const workspace = agent.workspace === LOCAL_WS.id ? LOCAL_WS : REMOTE_WS;
	render(
		<ServicesProvider source={fake.source}>
			<FixedNow value={NOW}>
				<WorkspacePorts agent={agent} workspace={workspace} />
				<PortOffers agent={agent} />
				{settings ? <PortPromptsSection /> : null}
			</FixedNow>
		</ServicesProvider>
	);
	return fake;
};

const card = (number: number) => screen.findByRole("region", { name: `Port ${number}` });

const stack = () => screen.queryByRole("region", { name: "Ports" });

/** Ports 3000 up, each first seen a second after the one before. */
const many = (count: number, over: Partial<ListeningPort> = {}) =>
	Array.from({ length: count }, (_, at) =>
		port(`p${at + 1}`, 3000 + at, { first_seen: NOW - (count - at) * 1000, ...over })
	);

const openPopover = async (name = "Workspace ws-maxhq on devbox") => {
	fireEvent.click(await screen.findByRole("button", { name }));
	return screen.findByRole("dialog");
};

describe("port offers", () => {
	it("offers a forward when the agent's process starts listening, and the forward moves to the header", async () => {
		const fake = show(remoteAgent);
		await screen.findByRole("button", { name: "Workspace ws-maxhq on devbox" });
		expect(stack()).not.toBeInTheDocument();

		fake.upsert([port("p1", 5173)]);
		const offer = await card(5173);
		expect(offer).toHaveTextContent("MaxHQ on devbox is listening on port 5173.");
		expect(offer).toHaveTextContent("Forward to localhost:5173?");
		// One port is one card, with no strip under it.
		expect(screen.queryByRole("button", { name: "Show all" })).not.toBeInTheDocument();
		expect(screen.queryByText(/more port/)).not.toBeInTheDocument();

		fireEvent.click(within(offer).getByRole("button", { name: "Forward" }));
		expect(fake.source.openForward).toHaveBeenCalledWith("p1");
		fake.upsert([forward("fwd-1", "p1", 5173)]);
		await waitFor(() => expect(stack()).not.toBeInTheDocument());

		const popup = await openPopover("Workspace ws-maxhq on devbox, 1 forwarded port");
		const forwarded = within(popup).getByRole("region", { name: "Forwarded ports" });
		fireEvent.click(
			within(forwarded).getByRole("button", { name: "Open localhost:5173 in the browser" })
		);
		expect(fake.source.openLocalhost).toHaveBeenCalledWith(5173);
	});

	it("shows only the agent's own ports, and none of the other agent's", async () => {
		show(remoteAgent, [port("p1", 5173), port("p2", 8787, { agent: otherAgent.id })]);
		await card(5173);
		expect(screen.queryByRole("region", { name: "Port 8787" })).not.toBeInTheDocument();
		expect(screen.queryByText(/more port/)).not.toBeInTheDocument();
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
		expect(stack()).not.toBeInTheDocument();

		const popup = await openPopover();
		const listening = within(popup).getByRole("region", { name: "Listening, not forwarded" });
		expect(listening).toHaveTextContent("5173");
		expect(within(listening).getByRole("button", { name: "Forward port 5173" })).toBeEnabled();
	});

	it("stacks the rest under the newest port, and names the first four", async () => {
		show(remoteAgent, many(7));
		const offer = await card(3006);
		expect(offer).toHaveTextContent("MaxHQ on devbox is listening on port 3006.");
		expect(screen.getAllByRole("region", { name: /^Port \d+$/ })).toHaveLength(1);

		const ports = stack();
		expect(ports).toHaveTextContent("6 more ports");
		const chips = within(ports!)
			.getAllByRole("button", { name: /^Show port / })
			.map((chip) => chip.textContent);
		expect(chips).toEqual(["3005", "3004", "3003", "3002"]);
		expect(ports).toHaveTextContent("+2");
	});

	it("lists the rest in a scrolling list on Show all, and folds them again on Hide", async () => {
		const fake = show(remoteAgent, many(7));
		await card(3006);
		const toggle = screen.getByRole("button", { name: "Show all" });
		expect(toggle).toHaveAttribute("aria-expanded", "false");
		fireEvent.click(toggle);

		const list = screen.getByRole("list", { name: "6 more ports" });
		expect(list).toHaveClass("max-h-[220px]", "overflow-y-auto");
		const rows = within(list).getAllByRole("listitem");
		expect(rows.map((row) => row.textContent?.slice(0, 4))).toEqual([
			"3005",
			"3004",
			"3003",
			"3002",
			"3001",
			"3000",
		]);
		expect(screen.queryByRole("button", { name: /^Show port / })).not.toBeInTheDocument();
		expect(screen.getByRole("button", { name: "Hide" })).toHaveAttribute(
			"aria-expanded",
			"true"
		);

		fireEvent.click(within(list).getByRole("button", { name: "Forward port 3001" }));
		expect(fake.source.openForward).toHaveBeenCalledWith("p2");
		fireEvent.click(within(list).getByRole("button", { name: "Dismiss port 3000" }));
		expect(
			within(screen.getByRole("list", { name: "5 more ports" })).getAllByRole("listitem")
		).toHaveLength(5);

		fireEvent.click(screen.getByRole("button", { name: "Hide" }));
		expect(screen.queryByRole("list", { name: /more port/ })).not.toBeInTheDocument();
		expect(screen.getAllByRole("button", { name: /^Show port / })).toHaveLength(4);
	});

	it("opens the list from a port named in the strip", async () => {
		show(remoteAgent, many(3));
		await card(3002);
		fireEvent.click(screen.getByRole("button", { name: "Show port 3000" }));
		expect(
			within(screen.getByRole("list", { name: "2 more ports" })).getAllByRole("listitem")
		).toHaveLength(2);
	});

	it("forwards every waiting port at once", async () => {
		const fake = show(remoteAgent, many(7));
		await card(3006);
		fireEvent.click(screen.getByRole("button", { name: "Forward all" }));
		expect(fake.source.openForward.mock.calls.map(([id]) => id).sort()).toEqual([
			"p1",
			"p2",
			"p3",
			"p4",
			"p5",
			"p6",
			"p7",
		]);
	});

	it("dismisses every waiting port at once, and keeps them in the popover", async () => {
		show(remoteAgent, many(7));
		await card(3006);
		fireEvent.click(screen.getByRole("button", { name: "Dismiss all" }));
		expect(stack()).not.toBeInTheDocument();

		const popup = await openPopover();
		const listening = within(popup).getByRole("region", { name: "Listening, not forwarded" });
		expect(within(listening).getAllByRole("button", { name: /^Forward port / })).toHaveLength(
			7
		);
	});

	it("takes a forwarded port out of the stack and leaves the rest", async () => {
		const fake = show(remoteAgent, many(3));
		await card(3002);
		fake.upsert([forward("fwd-1", "p3", 3002)]);
		expect(await card(3001)).toHaveTextContent("listening on port 3001.");
		expect(stack()).toHaveTextContent("1 more port");
	});

	it("turns off with the Port prompts setting, while the header still offers Forward", async () => {
		const fake = show(remoteAgent, many(2), { settings: true });
		await card(3001);
		const toggle = screen.getByRole("switch", { name: "Port prompts" });
		expect(toggle).toBeChecked();

		fireEvent.click(toggle);
		expect(stack()).not.toBeInTheDocument();
		expect(localStorage.getItem(PORT_PROMPTS_KEY)).toBe("off");
		expect(
			screen.getByText(/^Offer to forward or open a port.*Off\. Ports still show/)
		).toBeInTheDocument();

		const popup = await openPopover();
		const listening = within(popup).getByRole("region", { name: "Listening, not forwarded" });
		fireEvent.click(within(listening).getByRole("button", { name: "Forward port 3001" }));
		expect(fake.source.openForward).toHaveBeenCalledWith("p2");
	});

	it("stays off in a window opened after the setting was turned off", async () => {
		localStorage.setItem(PORT_PROMPTS_KEY, "off");
		show(remoteAgent, [port("p1", 5173)]);
		await screen.findByRole("button", { name: "Workspace ws-maxhq on devbox" });
		expect(stack()).not.toBeInTheDocument();
	});

	it("stops a forward from the popover and keeps the offer away after", async () => {
		const fake = show(remoteAgent, [port("p1", 5173), forward("fwd-1", "p1", 5173)]);
		const popup = await openPopover("Workspace ws-maxhq on devbox, 1 forwarded port");
		expect(stack()).not.toBeInTheDocument();
		const forwarded = within(popup).getByRole("region", { name: "Forwarded ports" });
		fireEvent.click(within(forwarded).getByRole("button", { name: "Stop forwarding 5173" }));
		expect(fake.source.closeForward).toHaveBeenCalledWith("fwd-1");
		fake.remove([forward("fwd-1", "p1", 5173)]);
		const listening = await within(popup).findByRole("region", {
			name: "Listening, not forwarded",
		});
		expect(listening).toHaveTextContent("5173");
		expect(stack()).not.toBeInTheDocument();
	});

	it("goes when the port closes, and says when a forward stopped with it", async () => {
		const fake = show(remoteAgent, [
			port("p1", 5173),
			forward("fwd-1", "p1", 5174),
			port("p2", 9229, { process: { pid: 9, name: "node", command: "node --inspect" } }),
		]);
		await card(9229);
		expect(screen.queryByRole("region", { name: "Port 5173" })).not.toBeInTheDocument();

		// Both close. The one with no forward just goes.
		fake.upsert([port("p1", 5173, { gone_since: NOW }), port("p2", 9229, { gone_since: NOW })]);
		await waitFor(() =>
			expect(screen.queryByRole("region", { name: "Port 9229" })).not.toBeInTheDocument()
		);
		const notice = "Port 5173 closed on devbox. Forwarding to localhost:5174 stopped.";
		expect(screen.getByText(notice)).toBeInTheDocument();

		// The notice outlives the port and its forward, until dismissed.
		fake.remove([port("p1", 5173), forward("fwd-1", "p1", 5174), port("p2", 9229)]);
		expect(screen.getByText(notice)).toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Dismiss the notice about port 5173" }));
		expect(screen.queryByText(notice)).not.toBeInTheDocument();
		expect(stack()).not.toBeInTheDocument();
	});

	it("keeps to one notice when several forwards stop, under the stack", async () => {
		const fake = show(remoteAgent, [
			port("p1", 5173),
			forward("fwd-1", "p1", 5173),
			port("p2", 8080),
			forward("fwd-2", "p2", 8080),
			port("p3", 9229),
		]);
		await card(9229);
		fake.upsert([port("p1", 5173, { gone_since: NOW }), port("p2", 8080, { gone_since: NOW })]);
		const latest = await screen.findByText(
			"Port 8080 closed on devbox. Forwarding to localhost:8080 stopped."
		);
		expect(stack()).toContainElement(latest);
		expect(screen.queryByText(/^Port 5173 closed/)).not.toBeInTheDocument();

		fireEvent.click(screen.getByRole("button", { name: "Dismiss the notice about port 8080" }));
		expect(screen.queryByText(/closed on devbox/)).not.toBeInTheDocument();
		expect(await card(9229)).toBeInTheDocument();
	});

	it("asks again when the port opens again", async () => {
		const fake = show(remoteAgent, [port("p1", 5173), forward("fwd-1", "p1", 5173)]);
		await screen.findByRole("button", {
			name: "Workspace ws-maxhq on devbox, 1 forwarded port",
		});
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
		await screen.findByRole("button", {
			name: "Workspace ws-maxhq on devbox, 1 forwarded port",
		});
		expect(stack()).not.toBeInTheDocument();

		fake.upsert([{ ...offline, state: { state: "connected", since: NOW } }]);
		const offer = await card(5173);
		expect(within(offer).getByRole("button", { name: "Forward" })).toBeInTheDocument();
		expect(screen.queryByRole("region", { name: "Port 8080" })).not.toBeInTheDocument();
		expect(
			screen.getByRole("button", { name: "Workspace ws-maxhq on devbox, 1 forwarded port" })
		).toBeInTheDocument();
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
		const popup = await openPopover();
		expect(popup).toHaveTextContent("devbox doesn't forward ports.");
		const listening = within(popup).getByRole("region", { name: "Listening, not forwarded" });
		expect(listening).toHaveTextContent("5173");
		expect(within(listening).queryByRole("button")).not.toBeInTheDocument();
		const others = within(popup).getByRole("region", { name: "Other ports" });
		expect(within(others).queryByRole("button")).not.toBeInTheDocument();
		expect(stack()).not.toBeInTheDocument();
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

	it("stacks a Mac agent's ports with Open on each and no Forward all", async () => {
		const fake = show(localAgent, many(3, { workspace: LOCAL_WS.id, agent: localAgent.id }));
		await card(3002);
		expect(stack()).toHaveTextContent("2 more ports");
		expect(screen.queryByRole("button", { name: "Forward all" })).not.toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Show all" }));
		const list = screen.getByRole("list", { name: "2 more ports" });
		fireEvent.click(
			within(list).getByRole("button", { name: "Open localhost:3001 in the browser" })
		);
		expect(fake.source.openLocalhost).toHaveBeenCalledWith(3001);
		expect(within(list).queryByRole("button", { name: /^Forward/ })).not.toBeInTheDocument();
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
