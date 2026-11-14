// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity } from "../bindings";
import { mockTestHosts } from "../data/mock-test-hosts";
import { DEVBOX, MAC } from "../data/mock-view";
import { entities, link, MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	couldHost,
	kindLabel,
	leaseOfHost,
	leasesOf,
	leasesOn,
	minutesLeft,
	NO_TEST_HOSTS,
	offersTestHosts,
	sameTestHostSettings,
	testHostSettingsOf,
	testHostsOn,
} from "./hosts";

const viewWith = (extra: Entity[] = []) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				links: [
					link({
						id: "mac",
						device: MAC,
						capabilities: {
							transfers: true,
							tunnels: true,
							workspaces: true,
							test_hosts: true,
						},
					}),
					link({ id: "box", device: DEVBOX }),
				],
			}),
			...mockTestHosts({ now: () => NOW }).entities(),
			...extra,
		],
	});

describe("test host selectors", () => {
	it("says which devices offer test hosts", () => {
		const view = viewWith();
		expect(offersTestHosts(view, MAC)).toBe(true);
		expect(offersTestHosts(view, DEVBOX)).toBe(false);
		expect(offersTestHosts(view, "dev-gone")).toBe(false);
	});

	it("lists a device's hosts in kind order", () => {
		const view = viewWith();
		expect(testHostsOn(view, MAC).map((h) => h.kind)).toEqual([
			"ios_simulator",
			"android_emulator",
			"macos",
		]);
		expect(testHostsOn(view, DEVBOX).map((h) => kindLabel(h.kind))).toEqual(["Linux"]);
	});

	it("finds leases by device, holder and host, leaving ended ones out", () => {
		const [held] = leasesOn(viewWith(), MAC);
		const view = viewWith([
			{ entity: "test_lease", ...held!, id: "lease-old", state: "ended", ended: "closed" },
		]);
		expect(leasesOn(view, MAC).map((l) => l.id)).toEqual(["lease-ios"]);
		expect(leasesOn(view, DEVBOX)).toEqual([]);
		expect(
			leasesOf(view, { kind: "agent", agent: "agent-signed-bundle" }).map((l) => l.id)
		).toEqual(["lease-ios"]);
		expect(leasesOf(view, { kind: "key", key: "agent-signed-bundle" })).toEqual([]);
		expect(leaseOfHost(view, "host-mac-ios-vm")?.id).toBe("lease-ios");
		expect(leaseOfHost(view, "host-mac-android")).toBeUndefined();
	});

	it("counts the minutes left by whichever end comes first", () => {
		const [lease] = leasesOn(viewWith(), MAC);
		expect(minutesLeft(lease!, NOW)).toBe(8);
		expect(minutesLeft(lease!, NOW + 30 * MIN)).toBe(0);
	});

	it("fills settings from before test hosts with the defaults", () => {
		expect(testHostSettingsOf(viewWith(), MAC)).toEqual(NO_TEST_HOSTS);
		expect(sameTestHostSettings({}, NO_TEST_HOSTS)).toBe(true);
		expect(
			sameTestHostSettings({ kinds: ["linux", "macos"] }, { kinds: ["macos", "linux"] })
		).toBe(true);
		expect(sameTestHostSettings({ kinds: ["linux"] }, { kinds: ["macos"] })).toBe(false);
		expect(sameTestHostSettings({ max_vms: 2 }, {})).toBe(false);
	});

	it("knows which OS could host each kind", () => {
		expect(couldHost("macos", "ios_simulator")).toBe(true);
		expect(couldHost("linux", "ios_simulator")).toBe(false);
		expect(couldHost("linux", "linux")).toBe(true);
		expect(couldHost("linux", "android_emulator")).toBe(true);
		expect(couldHost("linux", "macos")).toBe(false);
		expect(couldHost("macos", "linux")).toBe(false);
	});
});
