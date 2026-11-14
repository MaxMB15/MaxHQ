// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import { createNetworkTeller, type NetState, phoneNetworkOf, phoneOnline } from "./phone-network";

const on = (type: string): NetState => ({ type, isConnected: true });
const OFF: NetState = { type: "none", isConnected: false };
const UNKNOWN: NetState = { type: "unknown", isConnected: null };

describe("phoneNetworkOf", () => {
	it("reads Wi-Fi, cellular and none, with ethernet as Wi-Fi", () => {
		expect(phoneNetworkOf(on("wifi"))).toBe("wifi");
		expect(phoneNetworkOf(on("ethernet"))).toBe("wifi");
		expect(phoneNetworkOf(on("cellular"))).toBe("cellular");
		expect(phoneNetworkOf(OFF)).toBe("none");
		expect(phoneNetworkOf({ type: "wifi", isConnected: false })).toBe("none");
	});

	it("can't tell before NetInfo knows, or through a kind the core doesn't take", () => {
		expect(phoneNetworkOf(UNKNOWN)).toBeNull();
		expect(phoneNetworkOf(on("vpn"))).toBeNull();
	});
});

describe("phoneOnline", () => {
	it("is offline only once NetInfo knows", () => {
		expect(phoneOnline(OFF)).toBe(false);
		expect(phoneOnline(UNKNOWN)).toBe(true);
		expect(phoneOnline(on("cellular"))).toBe(true);
	});
});

describe("createNetworkTeller", () => {
	it("tells each new kind once, and nothing it can't tell", () => {
		const tell = vi.fn();
		const network = createNetworkTeller(tell, true);
		network.network(UNKNOWN);
		network.network(on("wifi"));
		network.network(on("wifi"));
		network.network(UNKNOWN);
		network.network(on("wifi"));
		network.network(on("cellular"));
		network.network(OFF);
		expect(tell.mock.calls).toEqual([["wifi"], ["cellular"], ["none"]]);
	});

	it("waits while the app is away, then tells the newest kind on its return", () => {
		const tell = vi.fn();
		const network = createNetworkTeller(tell, false);
		network.network(on("wifi"));
		network.network(on("cellular"));
		expect(tell).not.toHaveBeenCalled();
		network.active(true);
		expect(tell.mock.calls).toEqual([["cellular"]]);
		network.active(false);
		network.active(true);
		expect(tell).toHaveBeenCalledTimes(1);
	});
});
