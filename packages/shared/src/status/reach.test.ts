// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { type DeviceLink, PROTOCOL_VERSION } from "../bindings";
import { agent, entities, link, MIN, NOW, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	agentSynced,
	connectionBanner,
	deviceReach,
	linkRoutes,
	linkSynced,
	NO_LONGER_KNOWN,
	routeLabel,
	SIGN_IN_ENDED,
	stillSyncing,
	SYNC_GRACE_MS,
	versionNote,
} from "./reach";

const viewOf = (links: DeviceLink[]) => applySnapshot({ seq: 1, entities: entities({ links }) });

const down = (id: string): DeviceLink =>
	link({ id, state: { state: "unreachable", reason: "timed out", last_seen: NOW - 9 * MIN } });

describe("routeLabel", () => {
	it("writes a phone's route without its scheme", () => {
		expect(routeLabel({ type: "ssh", host: "ssh://dev@devbox:22" })).toBe("dev@devbox:22");
		expect(routeLabel({ type: "local" })).toBe("On this device");
	});

	it("names a device's network access by its address", () => {
		expect(routeLabel({ type: "ssh", host: "wss://192.168.1.20:7466" })).toBe(
			"Network, 192.168.1.20"
		);
		expect(routeLabel({ type: "ssh", host: "wss://[fd00::7]:7466" })).toBe("Network, fd00::7");
	});
});

describe("linkRoutes", () => {
	const route = { type: "ssh", host: "ssh://dev@devbox:22" } as const;
	const other = { type: "ssh", host: "ssh://dev@ws-alpha:22" } as const;

	it("falls back to the link's route when it lists none", () => {
		const one = [{ route, last_good_here: false }];
		expect(linkRoutes(link({ id: "a", route }))).toEqual(one);
		expect(linkRoutes(link({ id: "a", route, routes: [] }))).toEqual(one);
	});

	it("keeps the routes it lists, in order", () => {
		const routes = [
			{ route: other, last_good_here: true },
			{ route, last_good_here: false },
		];
		expect(linkRoutes(link({ id: "a", route, routes }))).toEqual(routes);
	});
});

describe("versionNote", () => {
	it("says when a device's maxd is newer or older than this app", () => {
		expect(versionNote(link({ id: "devbox", protocol: PROTOCOL_VERSION + 1 }))).toBe(
			"maxd on devbox is newer than this app; update the app for its new features"
		);
		expect(versionNote(link({ id: "devbox", protocol: PROTOCOL_VERSION - 1 }))).toBe(
			"maxd on devbox is older than this app; update it for the app's new features"
		);
	});

	it("says nothing when it's the same or the device hasn't said", () => {
		expect(versionNote(link({ id: "devbox", protocol: PROTOCOL_VERSION }))).toBeNull();
		expect(versionNote(link({ id: "devbox", protocol: null }))).toBeNull();
		expect(versionNote(link({ id: "devbox" }))).toBeNull();
	});
});

describe("deviceReach", () => {
	it("notes a device whose maxd is newer", () => {
		const view = viewOf([
			link({ id: "devbox", protocol: PROTOCOL_VERSION + 1 }),
			link({ id: "ws-alpha", protocol: PROTOCOL_VERSION }),
		]);
		expect(deviceReach(view, NOW).map((d) => d.version)).toEqual([
			"maxd on devbox is newer than this app; update the app for its new features",
			null,
		]);
	});

	it("says which devices answer and by which route", () => {
		const view = viewOf([
			link({
				id: "ws-alpha",
				route: { type: "ssh", host: "ssh://dev@ws-alpha.local:22" },
				routes: [
					{
						route: { type: "ssh", host: "ssh://dev@ws-alpha.local:22" },
						last_good_here: true,
					},
					{
						route: { type: "ssh", host: "ssh://dev@10.0.0.2:22" },
						last_good_here: false,
					},
				],
			}),
			down("devbox"),
		]);
		const [box, alpha] = deviceReach(view, NOW);
		expect(box).toMatchObject({
			name: "devbox",
			reached: false,
			via: null,
			look: "unreachable",
		});
		expect(box!.routes).toEqual([{ label: "max@devbox", current: true, lastGoodHere: false }]);
		expect(alpha).toMatchObject({ reached: true, via: "dev@ws-alpha.local:22", look: "live" });
		expect(alpha!.routes.map((r) => [r.label, r.current, r.lastGoodHere])).toEqual([
			["dev@ws-alpha.local:22", true, true],
			["dev@10.0.0.2:22", false, false],
		]);
		expect([box!.pairAgain, alpha!.pairAgain]).toEqual([false, false]);
	});

	it("says to pair again once a device ends the phone's sign-in", () => {
		const ended = link({
			id: "devbox",
			route: { type: "ssh", host: "wss://192.168.1.20:7466" },
			state: { state: "unreachable", reason: SIGN_IN_ENDED, last_seen: NOW - 9 * MIN },
		});
		const [device] = deviceReach(viewOf([ended]), NOW);
		expect(device).toMatchObject({ title: "Pair again", pairAgain: true, look: "unreachable" });
		expect(device!.detail).toMatch(
			/^The device ended this phone's sign-in, so pair the phone again\./
		);
	});

	it("says to pair again once a device no longer knows the phone's key", () => {
		const unknown = link({
			id: "devbox",
			state: {
				state: "unreachable",
				reason: `devbox ${NO_LONGER_KNOWN} Home Mac.`,
				last_seen: NOW - 9 * MIN,
			},
		});
		const [device] = deviceReach(viewOf([unknown]), NOW);
		expect(device).toMatchObject({ title: "Pair again", pairAgain: true });
		expect(device!.detail).toMatch(
			/^devbox no longer knows this phone\. Pair it again from Home Mac\. Last reachable/i
		);
		// A Mac with no name says "the Mac".
		const unnamed = link({
			id: "devbox",
			state: {
				state: "unreachable",
				reason: `devbox ${NO_LONGER_KNOWN} the Mac.`,
				last_seen: null,
			},
		});
		expect(deviceReach(viewOf([unnamed]), NOW)[0]!.pairAgain).toBe(true);
		// The words alone, with no device before them, are some other reason.
		const other = link({
			id: "devbox",
			state: { state: "unreachable", reason: `${NO_LONGER_KNOWN} the Mac.`, last_seen: null },
		});
		expect(deviceReach(viewOf([other]), NOW)[0]!.pairAgain).toBe(false);
	});
});

describe("connectionBanner", () => {
	it("shows nothing while a device answers, or with none", () => {
		expect(connectionBanner(viewOf([]))).toBeNull();
		expect(connectionBanner(viewOf([link({ id: "a" }), down("b")]))).toBeNull();
	});

	it("says it's reconnecting while it tries", () => {
		const trying = link({
			id: "devbox",
			state: { state: "connecting", step: "dialing", attempt: 2 },
		});
		expect(connectionBanner(viewOf([trying, down("b")]))).toMatchObject({
			tone: "reconnecting",
			title: "Reconnecting to devbox",
		});
		const fresh = link({ id: "c", state: { state: "unknown" } });
		expect(connectionBanner(viewOf([trying, fresh]))?.title).toBe("Reconnecting to 2 devices");
	});

	it("shows saved data when nothing answers, and offline when it knows", () => {
		expect(connectionBanner(viewOf([down("a"), down("b")]))).toMatchObject({
			tone: "cached",
			title: "Can't reach your devices",
		});
		expect(connectionBanner(viewOf([down("a")]), { online: false })?.tone).toBe("offline");
	});

	it("says when a device last answered, from the newest any link saw", () => {
		const older = link({
			id: "b",
			state: { state: "unreachable", reason: "timed out", last_seen: NOW - 40 * MIN },
		});
		const never = link({
			id: "c",
			state: { state: "unreachable", reason: "timed out", last_seen: null },
		});
		const now = NOW + 5 * MIN;
		expect(connectionBanner(viewOf([older, down("a"), never]), { now })?.body).toBe(
			"Last answered 14 min ago. MaxHQ keeps trying in the background."
		);
		expect(connectionBanner(viewOf([down("a")]), { online: false, now })?.body).toBe(
			"Last answered 14 min ago. MaxHQ reconnects once you're back."
		);
		expect(connectionBanner(viewOf([never]), { now })?.body).toBe(
			"What you see was saved when it last answered. MaxHQ keeps trying in the background."
		);
	});
});

describe("stillSyncing", () => {
	const fresh = (id: string): DeviceLink =>
		link({ id, state: { state: "connected", since: NOW - SYNC_GRACE_MS + 1 } });

	it("waits on a link that hasn't tried, is dialing, or just connected", () => {
		expect(stillSyncing(viewOf([link({ id: "a", state: { state: "unknown" } })]), NOW)).toBe(
			true
		);
		const dialing = link({
			id: "a",
			state: { state: "connecting", step: "dialing", attempt: 1 },
		});
		expect(stillSyncing(viewOf([dialing]), NOW)).toBe(true);
		expect(stillSyncing(viewOf([link({ id: "a" }), fresh("b")]), NOW)).toBe(true);
		expect(linkSynced(fresh("b"), NOW)).toBe(false);
	});

	it("is done once every link has synced or can't reach its device", () => {
		expect(stillSyncing(viewOf([link({ id: "a" }), down("b")]), NOW)).toBe(false);
		expect(stillSyncing(viewOf([fresh("b")]), NOW + 1)).toBe(false);
		expect(linkSynced(down("b"), NOW)).toBe(false);
		expect(stillSyncing(viewOf([]), NOW)).toBe(false);
	});
});

describe("agentSynced", () => {
	const viewWith = (state: DeviceLink["state"]) =>
		applySnapshot({
			seq: 1,
			entities: entities({
				agents: [agent({ id: "a1" })],
				workspaces: [workspace({ id: "w1" })],
				links: [link({ id: "mac", device: "dev-mac", state })],
			}),
		});

	it("holds once the agent's device has had time to sync", () => {
		const view = viewWith({ state: "connected", since: NOW - SYNC_GRACE_MS });
		expect(agentSynced(view, "a1", NOW)).toBe(true);
		expect(agentSynced(view, "a9", NOW)).toBe(false);
		expect(agentSynced(viewWith({ state: "unknown" }), "a1", NOW)).toBe(false);
	});
});
