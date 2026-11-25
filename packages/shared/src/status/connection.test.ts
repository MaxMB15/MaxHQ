// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AgentLink, LinkState, NetworkState } from "../bindings";
import { agent, device, link, MIN, network, NOW } from "../test/build";

import {
	agentConnection,
	CONNECTION_LABEL,
	deviceConnection,
	linkChecks,
	networkAddress,
	worstLook,
	type ConnectionLook,
} from "./connection";

const states: [LinkState, ConnectionLook, string][] = [
	[{ state: "connected", since: NOW - MIN }, "live", "Connected to devbox"],
	[{ state: "connecting", step: "dialing", attempt: 1 }, "connecting", "Connecting to devbox"],
	[{ state: "connecting", step: "dialing", attempt: 2 }, "connecting", "Reconnecting to devbox"],
	[
		{ state: "connecting", step: "installing_maxd", attempt: 1 },
		"connecting",
		"Installing maxd on devbox",
	],
	[
		{ state: "connecting", step: "handshake", attempt: 1 },
		"connecting",
		"Connecting to maxd on devbox",
	],
	[
		{ state: "connecting", step: "waiting_for_reboot", attempt: 3 },
		"connecting",
		"Waiting for devbox to restart",
	],
	[
		{ state: "connecting", step: "waiting_for_approval", attempt: 1 },
		"connecting",
		"Waiting for approval on devbox",
	],
	[
		{ state: "unreachable", reason: "auth failed", last_seen: null },
		"unreachable",
		"Can't reach devbox",
	],
	[{ state: "unknown" }, "unknown", "Not checked yet"],
];

const nl = (state: LinkState) =>
	link({ id: "devbox", state, network: "tailnet", peer: null, device: "dev-nl" });

describe("device connection", () => {
	it.each(states)("maps %j to its look and hover", (state, look, title) => {
		const conn = deviceConnection({ link: nl(state), network: network() }, NOW);
		expect(conn.look).toBe(look);
		expect(conn.card.title).toBe(title);
		expect(CONNECTION_LABEL[look]).toBeTruthy();
	});

	it("always gives a reason when a device can't be reached", () => {
		const conn = deviceConnection(
			{
				link: nl({
					state: "unreachable",
					reason: "Tailscale is off",
					last_seen: NOW - 131 * MIN,
				}),
			},
			NOW
		);
		expect(conn.card.body).toBe("Tailscale is off. Last reachable at 12:37, 2 h 11 min ago.");
	});

	it("shows the check bar while an attempt runs", () => {
		const conn = deviceConnection(
			{
				link: nl({ state: "connecting", step: "installing_maxd", attempt: 1 }),
				network: network(),
			},
			NOW
		);
		expect(conn.card.checks?.map((c) => [c.short, c.state])).toEqual([
			["Net", "passed"],
			["Tailnet", "passed"],
			["Peer", "passed"],
			["Ping", "passed"],
			["SSH", "passed"],
			["maxd", "running"],
			["Agent", "unknown"],
		]);
	});

	it("shares a jump host's problem", () => {
		const bastion = link({
			id: "bastion",
			state: { state: "unreachable", reason: "offline", last_seen: null },
		});
		const behind = link({ id: "inner", state: { state: "unknown" }, via: ["max@bastion"] });
		const conn = deviceConnection({ link: behind, jumpHosts: [bastion] }, NOW);
		expect(conn.look).toBe("unreachable");
		expect(conn.card.body).toContain("bastion");
	});

	it("names the route a phone reaches a device's network access by", () => {
		const phone = link({
			id: "devbox",
			route: { type: "ssh", host: "wss://192.168.1.20:7466" },
		});
		const ssh = link({ id: "devbox", route: { type: "ssh", host: "max@devbox" } });
		const route = (l: typeof phone) =>
			deviceConnection({ link: l }, NOW).card.facts?.find((f) => f.label === "Route")?.value;
		expect(route(phone)).toBe("Network, 192.168.1.20");
		expect(route(ssh)).toBe("SSH as max@devbox");
	});
});

describe("networkAddress", () => {
	it("reads the address out of a network route alone", () => {
		expect(networkAddress("wss://192.168.1.20:7466")).toBe("192.168.1.20");
		expect(networkAddress("wss://[fd00::7]:7466")).toBe("fd00::7");
		expect(networkAddress("wss://devbox.example.ts.net:7466")).toBe("devbox.example.ts.net");
		expect(networkAddress("ssh://dev@devbox:22")).toBeNull();
		expect(networkAddress("max@devbox")).toBeNull();
	});
});

describe("agent connection", () => {
	const ctx = {
		link: nl({ state: "connected", since: NOW - MIN }),
		device: device({ id: "dev-nl" }),
	};

	const agentLinks: [AgentLink, ConnectionLook, string][] = [
		[{ state: "live" }, "live", "Live"],
		[
			{ state: "down", reason: "interrupted by a restart", since: NOW - 7 * MIN },
			"agent_down",
			"Agent not running",
		],
		[{ state: "unknown" }, "unknown", "Agent state unknown"],
	];

	it.each(agentLinks)("on a connected device maps %j to its look", (agentLink, look, title) => {
		const conn = agentConnection(agent({ id: "a", link: agentLink }), ctx, NOW);
		expect(conn.look).toBe(look);
		expect(conn.card.title).toBe(title);
	});

	it("says why an agent doesn't run", () => {
		const conn = agentConnection(
			agent({ id: "a", link: { state: "down", reason: "exited", since: NOW - 7 * MIN } }),
			ctx,
			NOW
		);
		expect(conn.card.body).toBe("devbox answers, but the agent doesn't run. Exited at 14:41.");
	});

	it("takes the device's look when the device isn't connected", () => {
		const down = {
			link: nl({ state: "unreachable", reason: "Tailscale is off", last_seen: null }),
		};
		expect(agentConnection(agent({ id: "a" }), down, NOW).look).toBe("unreachable");
		expect(agentConnection(agent({ id: "a" }), undefined, NOW).look).toBe("unknown");
	});

	it("shows an agent whose idle CLI stopped as asleep, and says the next message wakes it", () => {
		const conn = agentConnection(agent({ id: "a", asleep: true, pid: null }), ctx, NOW);
		expect(conn.look).toBe("asleep");
		expect(conn.card).toMatchObject({
			title: "Asleep",
			body: "devbox stopped this agent's CLI after it sat idle. Your next message starts it again, with nothing lost.",
		});
		expect(conn.card.facts.map((f) => f.label)).not.toContain("Process");
		expect(agentConnection(agent({ id: "a", asleep: false }), ctx, NOW).look).toBe("live");
	});

	it("lets the device's state win over asleep", () => {
		const sleeper = agent({ id: "a", asleep: true });
		const down = {
			link: nl({ state: "unreachable", reason: "Tailscale is off", last_seen: null }),
		};
		expect(agentConnection(sleeper, down, NOW).look).toBe("unreachable");
		const reconnecting = { link: nl({ state: "connecting", step: "dialing", attempt: 2 }) };
		expect(agentConnection(sleeper, reconnecting, NOW).look).toBe("connecting");
		const gone = {
			...sleeper,
			link: { state: "down", reason: "exited", since: null },
		} as const;
		expect(agentConnection(gone, ctx, NOW).look).toBe("agent_down");
	});

	it("covers every look", () => {
		const looks = new Set([
			...states.map(([, l]) => l),
			...agentLinks.map(([, l]) => l),
			agentConnection(agent({ id: "a", asleep: true }), ctx, NOW).look,
		]);
		expect([...looks].sort()).toEqual(Object.keys(CONNECTION_LABEL).sort());
	});
});

describe("worstLook", () => {
	it("ranks not reachable worst, then live, and asleep best", () => {
		expect(worstLook(["live", "unknown", "agent_down", "connecting"])).toBe("agent_down");
		expect(worstLook(["live", "unreachable", "agent_down"])).toBe("unreachable");
		expect(worstLook(["live", "live"])).toBe("live");
		expect(worstLook(["live", "asleep"])).toBe("live");
		expect(worstLook(["asleep", "asleep"])).toBe("asleep");
		expect(worstLook(["asleep", "unknown"])).toBe("unknown");
		expect(worstLook([])).toBeUndefined();
	});
});

describe("linkChecks", () => {
	const netStates: [NetworkState, string][] = [
		["off", "Tailscale is off on this Mac"],
		["needs_login", "Tailscale wants you to log in again"],
		["needs_approval", "An admin has to approve this Mac on the tailnet"],
		["not_running", "Tailscale isn't running on this Mac"],
	];

	it.each(netStates)("stops at the tailnet when it's %s", (state, detail) => {
		const checks = linkChecks(
			{
				link: nl({ state: "unreachable", reason: "Tailscale is off", last_seen: null }),
				network: network({ state }),
			},
			NOW
		);
		expect(checks.find((c) => c.id === "tailnet")).toMatchObject({ state: "failed", detail });
		expect(checks.filter((c) => c.state === "not_run").map((c) => c.id)).toEqual([
			"peer",
			"ping",
			"ssh",
			"maxd",
		]);
	});

	it("stops at the peer when Tailscale says it's offline", () => {
		const checks = linkChecks(
			{
				link: {
					...nl({
						state: "unreachable",
						reason: "peer offline",
						last_seen: NOW - 46 * MIN,
					}),
					peer: {
						host_name: "devbox",
						dns_name: "devbox.tail1234.ts.net",
						online: false,
						last_seen: NOW - 46 * MIN,
						path: { type: "idle" },
						key_expiry: null,
					},
				},
				network: network(),
				agents: [agent({ id: "a" }), agent({ id: "b" })],
			},
			NOW
		);
		expect(checks.map((c) => c.state)).toEqual([
			"passed",
			"passed",
			"failed",
			"not_run",
			"not_run",
			"not_run",
			"unknown",
		]);
		expect(checks[2]?.detail).toContain("Offline since 14:02");
		expect(checks[6]?.detail).toBe("Unknown. 2 agents last seen here");
	});

	it("reaches a WSL distro on this PC through wsl.exe, with no network between", () => {
		const wsl = (via: string | null) =>
			linkChecks(
				{
					link: {
						...nl({ state: "connected", since: NOW - MIN }),
						network: null,
						route: { type: "wsl", distro: "Ubuntu", via },
					},
					device: device({ id: "dev-nl" }),
				},
				NOW
			);
		const here = wsl(null);
		expect(here.map((c) => [c.id, c.short])).toEqual([
			["ssh", "WSL"],
			["maxd", "maxd"],
			["agents", "Agent"],
		]);
		expect(here[0]?.detail).toBe("Connected as devbox");
		const there = wsl("dev@pc");
		expect(there.map((c) => [c.id, c.short])).toEqual([
			["net", "Net"],
			["ping", "Ping"],
			["ssh", "SSH"],
			["maxd", "maxd"],
			["agents", "Agent"],
		]);
		expect(there.find((c) => c.id === "ssh")?.detail).toBe("Connected as dev@pc");
	});

	it("passes every check on a connected device and counts its agents", () => {
		const checks = linkChecks(
			{
				link: nl({ state: "connected", since: NOW - MIN }),
				device: device({ id: "dev-nl" }),
				agents: [
					agent({ id: "a" }),
					agent({ id: "b", link: { state: "down", reason: "exited", since: null } }),
				],
			},
			NOW
		);
		expect(checks.every((c) => c.state === "passed" || c.state === "unknown")).toBe(true);
		expect(checks.find((c) => c.id === "maxd")?.detail).toBe("Answered, maxd 0.1.0");
		expect(checks.find((c) => c.id === "agents")?.detail).toBe("1 running, 1 not running");
	});

	it("counts asleep agents apart from running ones", () => {
		const checks = linkChecks(
			{
				link: nl({ state: "connected", since: NOW - MIN }),
				device: device({ id: "dev-nl" }),
				agents: [
					agent({ id: "a" }),
					agent({ id: "b", asleep: true }),
					agent({ id: "c", link: { state: "down", reason: "exited", since: null } }),
				],
			},
			NOW
		);
		expect(checks.find((c) => c.id === "agents")?.detail).toBe(
			"1 running, 1 asleep, 1 not running"
		);
	});

	it("checks only maxd and the agents on this Mac", () => {
		const checks = linkChecks(
			{
				link: link({ id: "mac", route: { type: "local" } }),
				device: device({ id: "dev-mac" }),
			},
			NOW
		);
		expect(checks.map((c) => c.id)).toEqual(["maxd", "agents"]);
	});
});
