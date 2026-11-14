// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, TestHost } from "../bindings";
import { mockSource } from "../data/mock-source";
import { MAC_ANDROID, MAC_IOS } from "../data/mock-test-hosts";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { deviceLive, kindPlaces, placesOn, suggestion, testHostDevices, withKind } from "./place";

const load = async (scenario?: string, edit = (e: Entity) => e) => {
	const snapshot = await mockSource({ now: () => NOW, tick: false, scenario }).snapshot();
	return applySnapshot({ ...snapshot, entities: snapshot.entities.map(edit) });
};

const vm = (state: TestHost["state"]): TestHost => ({
	id: "h",
	device: MAC,
	kind: "ios_simulator",
	backend: "mac_vm",
	state,
	setup: null,
	error: null,
	agent_device: null,
	last_reset: null,
	queue: 0,
	vm: null,
});

describe("test host placement", () => {
	it("lists each kind with the devices that could host it", async () => {
		const places = kindPlaces(await load());
		expect(places.map((k) => [k.kind, k.places.map((p) => [p.name, p.ticked])])).toEqual([
			["ios_simulator", [["Mac", true]]],
			[
				"android_emulator",
				[
					["devbox", false],
					["Mac", true],
				],
			],
			["linux", [["devbox", true]]],
			["macos", [["Mac", true]]],
		]);
		const android = places[1]!.places;
		expect(android[1]!.hosts.map((h) => h.id)).toEqual([MAC_ANDROID]);
		expect(android[0]!.hosts).toEqual([]);
	});

	it("leaves out devices whose maxd doesn't offer test hosts", async () => {
		const view = await load(undefined, (e) =>
			e.entity === "device_link" && e.device === DEVBOX
				? { ...e, capabilities: { ...e.capabilities!, test_hosts: false } }
				: e
		);
		expect(testHostDevices(view).map((d) => d.id)).toEqual([MAC]);
		expect(kindPlaces(view).find((k) => k.kind === "linux")?.places).toEqual([]);
		expect(placesOn(view, DEVBOX)).toEqual([]);
	});

	it("lists the kinds one device could host", async () => {
		const view = await load();
		expect(placesOn(view, MAC).map((p) => p.kind)).toEqual([
			"ios_simulator",
			"android_emulator",
			"macos",
		]);
		expect(placesOn(view, MAC)[0]!.hosts.map((h) => h.id)).toEqual([MAC_IOS]);
		expect(placesOn(view, DEVBOX).map((p) => p.kind)).toEqual(["android_emulator", "linux"]);
		expect(placesOn(view, "dev-gone")).toEqual([]);
	});

	it("suggests the Mac for Android and the VM for iOS once it's built", () => {
		expect(suggestion("android_emulator", "macos", [])).toMatch(/^Suggested/);
		expect(suggestion("android_emulator", "linux", [])).toBeNull();
		expect(suggestion("ios_simulator", "macos", [vm("warm")])).toMatch(/^Suggested/);
		expect(suggestion("ios_simulator", "macos", [vm("off")])).toBeNull();
		expect(
			suggestion("ios_simulator", "macos", [{ ...vm("warm"), backend: "mac_host" }])
		).toBeNull();
		expect(suggestion("linux", "linux", [])).toBeNull();
	});

	it("ticks and unticks a kind, keeping the kinds in order", () => {
		const settings = { kinds: ["macos" as const], max_vms: 2 };
		expect(withKind(settings, "ios_simulator", true)).toEqual({
			kinds: ["ios_simulator", "macos"],
			max_vms: 2,
		});
		expect(withKind(settings, "macos", false)).toEqual({ kinds: [], max_vms: 2 });
		expect(withKind({}, "linux", true)).toEqual({ kinds: ["linux"] });
		expect(withKind(settings, "macos", true).kinds).toEqual(["macos"]);
	});

	it("says whether a device answers now", async () => {
		expect(deviceLive(await load(), DEVBOX, NOW)).toBe(true);
		expect(deviceLive(await load("peer_offline"), DEVBOX, NOW)).toBe(false);
		expect(deviceLive(await load(), "dev-gone", NOW)).toBe(false);
	});
});
