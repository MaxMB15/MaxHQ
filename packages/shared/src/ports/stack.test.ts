// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { DeviceLink, Entity, ListeningPort } from "../bindings";
import { device, entities, link, MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import type { ClosedForward } from "./select";
import {
	latestNotice,
	newestFirst,
	offersPorts,
	portStack,
	readPortPrompts,
	waitingPorts,
	writePortPrompts,
} from "./stack";

const listening = (id: string, number: number, over: Partial<ListeningPort> = {}) => ({
	entity: "listening_port" as const,
	id,
	workspace: "ws-1",
	agent: "a1",
	port: number,
	bind: { type: "any" as const },
	process: { pid: 7, name: "node", command: "node server.js" },
	url_hint: null,
	first_seen: NOW - MIN,
	gone_since: null,
	...over,
});

const forward = (id: string, port: string): Entity => ({
	entity: "forward",
	id,
	port,
	client: "client-1",
	local_port: 5173,
	opened_at: NOW,
});

const view = (extra: Entity[], devbox: Partial<DeviceLink> = {}) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-devbox" })],
				links: [
					link({ id: "mac", device: "dev-mac", route: { type: "local" } }),
					link({ id: "devbox", device: "dev-devbox", ...devbox }),
				],
			}),
			...extra,
		],
	});

const ids = (ports: readonly ListeningPort[]) => ports.map((port) => port.id);

const ports = (count: number) =>
	Array.from({ length: count }, (_, at) =>
		listening(`p${at + 1}`, 3000 + at, { first_seen: NOW - (count - at) * MIN })
	);

describe("the port stack", () => {
	it("puts the port first seen last on top, and the later listed of two seen at once", () => {
		const order = newestFirst([
			listening("old", 4000, { first_seen: NOW - 5 * MIN }),
			listening("tie-first", 5173, { first_seen: NOW - MIN }),
			listening("tie-second", 8080, { first_seen: NOW - MIN }),
			listening("mid", 3000, { first_seen: NOW - 3 * MIN }),
		]);
		expect(ids(order)).toEqual(["tie-second", "tie-first", "mid", "old"]);
	});

	it("shows one card and no strip for one port", () => {
		const stack = portStack(ports(1));
		expect(stack?.newest.id).toBe("p1");
		expect(stack?.rest).toEqual([]);
		expect(stack?.chips).toEqual([]);
		expect(stack?.more).toBe(0);
	});

	it("names the first four of the rest and counts the others", () => {
		const stack = portStack(newestFirst(ports(7)));
		expect(stack?.newest.id).toBe("p7");
		expect(ids(stack?.rest ?? [])).toEqual(["p6", "p5", "p4", "p3", "p2", "p1"]);
		expect(ids(stack?.chips ?? [])).toEqual(["p6", "p5", "p4", "p3"]);
		expect(stack?.more).toBe(2);
	});

	it("names every one of the rest while they fit", () => {
		const stack = portStack(newestFirst(ports(4)));
		expect(stack?.chips).toHaveLength(3);
		expect(stack?.more).toBe(0);
	});

	it("is empty when no port waits", () => {
		expect(portStack([])).toBeNull();
	});

	it("leaves out ports that were forwarded, dismissed or closed, and other agents' ports", () => {
		const state = view([
			listening("p1", 3000, { first_seen: NOW - 4 * MIN }),
			listening("p2", 4000, { first_seen: NOW - 3 * MIN }),
			listening("p3", 5173, { first_seen: NOW - 2 * MIN }),
			listening("p4", 8080, { first_seen: NOW - MIN, gone_since: NOW }),
			listening("p5", 9229, { agent: "a2" }),
			listening("p6", 6006, { first_seen: NOW - 30_000 }),
			forward("fwd-1", "p2"),
		]);
		expect(ids(waitingPorts(state, "a1", new Set()))).toEqual(["p6", "p3", "p1"]);
		expect(ids(waitingPorts(state, "a1", new Set(["p6", "p1"])))).toEqual(["p3"]);
		expect(waitingPorts(state, "a1", new Set(["p1", "p3", "p6"]))).toEqual([]);
	});
});

describe("which devices get offers", () => {
	it("offers this Mac's ports and a reachable device's that forwards", () => {
		const state = view([]);
		expect(offersPorts(state, "dev-mac", NOW)).toBe(true);
		expect(offersPorts(state, "dev-devbox", NOW)).toBe(true);
	});

	it("offers nothing on a device that doesn't forward ports", () => {
		const state = view([], {
			capabilities: { transfers: true, tunnels: false, workspaces: true },
		});
		expect(offersPorts(state, "dev-devbox", NOW)).toBe(false);
	});

	it("offers nothing on a device that can't be reached", () => {
		const state = view([], {
			state: { state: "unreachable", reason: "timed out", last_seen: NOW - MIN },
		});
		expect(offersPorts(state, "dev-devbox", NOW)).toBe(false);
	});
});

describe("the notice that a forward stopped", () => {
	const notice = (agent: string, port: number): ClosedForward => ({
		agent,
		port,
		localPort: port,
		device: "dev-devbox",
	});

	it("keeps only the latest of the agent's notices", () => {
		const closed = [notice("a1", 5173), notice("a1", 8080), notice("a2", 3000)];
		expect(latestNotice(closed, "a1", [])).toEqual(notice("a1", 8080));
		expect(latestNotice(closed, "a3", [])).toBeNull();
	});

	it("drops a notice once its port listens again", () => {
		const closed = [notice("a1", 5173), notice("a1", 8080)];
		const reopened = [listening("p9", 8080)];
		expect(latestNotice(closed, "a1", reopened)).toEqual(notice("a1", 5173));
	});
});

describe("the Port prompts setting", () => {
	it("is on unless it was turned off", () => {
		expect(readPortPrompts(null)).toBe(true);
		expect(readPortPrompts("on")).toBe(true);
		expect(readPortPrompts(writePortPrompts(false))).toBe(false);
		expect(readPortPrompts(writePortPrompts(true))).toBe(true);
	});
});
