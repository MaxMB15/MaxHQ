// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { device, entities, link, MIN, network, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import { deviceNode } from "../view/select";
import { deviceChips, splitDetail } from "./device-page";

const view = applySnapshot({
	seq: 1,
	entities: entities({
		devices: [device({ id: "dev-box" }), device({ id: "dev-mac", os: "macos" })],
		networks: [network()],
		links: [
			link({ id: "box", name: "devbox", device: "dev-box", network: "tailnet" }),
			link({ id: "mac", name: "Home Mac", device: "dev-mac", route: { type: "local" } }),
			link({
				id: "gone",
				device: "dev-box",
				state: { state: "unreachable", reason: "timed out", last_seen: NOW - MIN },
			}),
		],
	}),
});

const chips = (id: string) => deviceChips(deviceNode(view, view.tables.device_link.get(id)!, NOW));

describe("deviceChips", () => {
	it("names the network, the route, the OS and maxd's version", () => {
		expect(chips("box")).toEqual(["Needs Tailscale", "SSH as max@box", "Linux", "maxd 0.1.0"]);
		expect(chips("mac")).toEqual(["This Mac", "macOS", "maxd 0.1.0"]);
	});

	it("says the version is from when it was last seen", () => {
		expect(chips("gone").at(-1)).toBe("maxd 0.1.0 when last seen");
	});
});

describe("splitDetail", () => {
	it("splits at the first full stop", () => {
		expect(splitDetail("No route. The host didn't answer. Try later")).toEqual([
			"No route",
			"The host didn't answer. Try later",
		]);
		expect(splitDetail("All fine")).toEqual(["All fine", undefined]);
	});
});
