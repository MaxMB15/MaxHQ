// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it } from "vitest";

import type { Entity, ListeningPort, Workspace } from "@maxhq/shared/bindings";
import type { ClosedForward } from "@maxhq/shared/ports/select";
import {
	agent,
	device,
	entities,
	link,
	network,
	NOW,
	project,
	workspace as buildWorkspace,
} from "@maxhq/shared/test/build";
import { applySnapshot, type ViewState } from "@maxhq/shared/view/replica";
import { createViewStore } from "@maxhq/shared/view/store";

import {
	clearMyForwards,
	dropForward,
	keepForward,
	myForward,
	myNotices,
	nameForwards,
	noticeForwards,
	offerCards,
	onForward,
	openPortCount,
	opensOutside,
	pageUrl,
	portSections,
	showsPorts,
	useMyForwards,
	watchMyForwards,
} from "./running";

afterEach(() => clearMyForwards());

const WS: Workspace = buildWorkspace({
	id: "ws-maxhq",
	project: "proj-maxhq",
	device: "dev-devbox",
	location: { type: "container", instance: "ws-maxhq", path: "/home/agent/work/MaxHQ" },
});

const lead = agent({ id: "a1", workspace: WS.id, control: "held" });
const other = agent({ id: "a2", workspace: WS.id, control: "held" });

const port = (id: string, number: number, over: Partial<ListeningPort> = {}): ListeningPort => ({
	id,
	workspace: WS.id,
	agent: lead.id,
	port: number,
	bind: { type: "any" },
	process: { pid: 412, name: "vite", command: "pnpm dev" },
	url_hint: null,
	first_seen: NOW - 4000,
	gone_since: null,
	...over,
});

const forward = (id: string, portId: string, local: number, opened = NOW): Entity => ({
	entity: "forward",
	id,
	port: portId,
	client: "client-1",
	local_port: local,
	opened_at: opened,
});

const viewOf = (ports: ListeningPort[], extra: Entity[] = []): ViewState =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				agents: [lead, other],
				devices: [device({ id: "dev-devbox" })],
				links: [link({ id: "devbox", device: "dev-devbox", network: "tailnet" })],
				networks: [network()],
				projects: [project("proj-maxhq", "MaxHQ")],
				workspaces: [WS],
			}),
			...ports.map((p): Entity => ({ entity: "listening_port", ...p })),
			...extra,
		],
	});

const mine = () => useMyForwards.getState().mine;

describe("the phone's own forwards", () => {
	it("counts a forward while the view has it, by ID or by its port here", () => {
		const p1 = port("p1", 5173);
		keepForward(p1, { forward: "fwd-1", local_port: 5173 });
		const kept = mine().get("p1");

		expect(myForward(viewOf([p1], [forward("fwd-1", "p1", 5173)]), kept)).toEqual({
			forward: "fwd-1",
			local: 5173,
		});
		// A reconnect gave it a new ID, on the same port here.
		expect(myForward(viewOf([p1], [forward("fwd-9", "p1", 5173)]), kept)).toEqual({
			forward: "fwd-9",
			local: 5173,
		});
		// It closed.
		expect(myForward(viewOf([p1]), kept)).toBeNull();
		expect(myForward(viewOf([p1]), undefined)).toBeNull();
	});

	it("waits for the device to name a forward, then follows it", () => {
		const p1 = port("p1", 5173);
		keepForward(p1, { forward: null, local_port: 5174 });
		expect(myForward(viewOf([p1]), mine().get("p1"))).toEqual({ forward: null, local: 5174 });

		const store = createViewStore();
		const stop = watchMyForwards(store);
		const named = viewOf([p1], [forward("fwd-1", "p1", 5174)]);
		store.setState({ view: named });
		expect(mine().get("p1")?.forward).toBe("fwd-1");
		// Once named, a forward that closes is gone rather than waiting.
		store.setState({ view: viewOf([p1]) });
		expect(myForward(viewOf([p1]), mine().get("p1"))).toBeNull();
		stop();
	});

	it("takes the newest forward on the same port here, not a Mac's older one", () => {
		const p1 = port("p1", 5173);
		keepForward(p1, { forward: null, local_port: 5173 });
		const view = viewOf(
			[p1],
			[forward("fwd-mac", "p1", 5173, NOW - 60_000), forward("fwd-phone", "p1", 5173, NOW)]
		);
		expect(myForward(view, mine().get("p1"))?.forward).toBe("fwd-phone");
	});

	it("leaves the map alone when there's nothing to name", () => {
		keepForward(port("p1", 5173), { forward: "fwd-1", local_port: 5173 });
		const before = mine();
		expect(nameForwards(viewOf([]), before)).toBe(before);
	});

	it("forgets a forward once it's stopped", () => {
		keepForward(port("p1", 5173), { forward: "fwd-1", local_port: 5173 });
		const before = mine();
		dropForward("p2");
		expect(mine()).toBe(before);
		dropForward("p1");
		expect(mine().size).toBe(0);
	});
});

describe("ports and processes", () => {
	it("sorts the workspace's ports into forwarded, listening and other", () => {
		const ports = [
			port("p1", 5173),
			port("p2", 9229),
			port("p3", 3000, { agent: other.id }),
			port("p4", 5432, {
				agent: null,
				process: { pid: 7, name: "postgres", command: "postgres" },
			}),
			port("p5", 8080, { gone_since: NOW - 1000 }),
		];
		keepForward(ports[0]!, { forward: "fwd-1", local_port: 5174 });
		// The Mac forwards 9229. That reaches only the Mac, so the phone offers its own.
		const view = viewOf(ports, [forward("fwd-1", "p1", 5174), forward("fwd-mac", "p2", 9229)]);
		const sections = portSections(view, WS, mine());

		expect(sections.forwarded.map((row) => [row.port.port, row.forward.local])).toEqual([
			[5173, 5174],
		]);
		expect(sections.listening.map((p) => p.port)).toEqual([3000, 9229]);
		expect(sections.others.map((p) => p.port)).toEqual([5432]);
	});

	it("offers each of the agent's ports above the composer, unless dismissed or not now", () => {
		const ports = [port("p1", 5173), port("p2", 9229), port("p3", 3000, { agent: other.id })];
		keepForward(ports[0]!, { forward: "fwd-1", local_port: 5173 });
		const view = viewOf(ports, [forward("fwd-1", "p1", 5173)]);

		const cards = offerCards(view, lead.id, mine(), new Set(), true);
		expect(cards.map((card) => [card.port.port, card.forward?.local ?? null])).toEqual([
			[5173, 5173],
			[9229, null],
		]);
		expect(offerCards(view, lead.id, mine(), new Set(["p2"]), true)).toHaveLength(1);
		// A device that can't take a forward now still shows the ones the phone has.
		expect(offerCards(view, lead.id, mine(), new Set(), false)).toHaveLength(1);
	});

	it("keeps the notices about stopped forwards to the ones this phone opened", () => {
		keepForward(port("p1", 5173), { forward: "fwd-1", local_port: 5174 });
		const notice = (over: Partial<ClosedForward>): ClosedForward => ({
			agent: lead.id,
			port: 5173,
			localPort: 5174,
			device: "dev-devbox",
			...over,
		});
		const closed = [
			notice({}),
			notice({ port: 9229, localPort: 9229 }),
			notice({ agent: other.id }),
		];
		expect(myNotices(closed, mine(), lead.id, [])).toEqual([notice({})]);
		// The port opened again, so its new offer stands in for the notice.
		expect(myNotices(closed, mine(), lead.id, [port("p9", 5173)])).toEqual([]);
		expect(noticeForwards(notice({}), mine())).toEqual(["p1"]);
	});

	it("answers what the More sheet asks", () => {
		const ports = [
			port("p1", 5173),
			port("p2", 9229),
			port("p3", 3000, { agent: other.id }),
			port("p4", 8080, { gone_since: NOW - 1000 }),
		];
		const view = viewOf(ports);
		expect(openPortCount(view, lead.id)).toBe(2);
		expect(openPortCount(view, "a-none")).toBe(0);
		expect(showsPorts(view, lead)).toBe(true);
		expect(showsPorts(view, agent({ id: "a9", workspace: "ws-gone" }))).toBe(false);
	});

	it("shows a forward at its IP address here, and keeps its page on it", () => {
		expect(pageUrl(5174)).toBe("http://127.0.0.1:5174");
		expect(onForward("http://127.0.0.1:5174/settings?tab=2", 5174)).toBe(true);
		expect(onForward("http://localhost:5174", 5174)).toBe(true);
		expect(onForward("http://[::1]:5174#top", 5174)).toBe(true);
		expect(onForward("about:blank", 5174)).toBe(true);
		expect(onForward("http://127.0.0.1:51740/", 5174)).toBe(false);
		expect(onForward("http://127.0.0.1:5173/", 5174)).toBe(false);
		expect(onForward("http://127.0.0.1.example.com:5174/", 5174)).toBe(false);
		expect(onForward("https://example.com/", 5174)).toBe(false);
		expect(opensOutside("https://example.com/")).toBe(true);
		expect(opensOutside("javascript:alert(1)")).toBe(false);
	});
});
