// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { TestHost, TestLease } from "../bindings";
import { NOW } from "../test/build";
import {
	DEVBOX_LINUX,
	IOS_LEASE,
	MAC_ANDROID,
	MAC_IOS,
	MAC_MACOS,
	mockTestHosts,
	noTestHosts,
} from "./mock-test-hosts";
import { DEVBOX, MAC } from "./mock-view";
import { TEST_HOSTS_NOT_SET_UP } from "./test-hosts";

const DONE = { status: "ok", data: { status: "done" } };

const setup = () => {
	const changed = vi.fn();
	const mock = mockTestHosts({ now: () => NOW, changed });
	const hostOf = (id: string) =>
		mock.entities().find((e) => e.entity === "test_host" && e.id === id) as
			TestHost | undefined;
	const leases = () =>
		mock.entities().filter((e) => e.entity === "test_lease") as unknown as TestLease[];
	return { ...mock, changed, hostOf, leases };
};

describe("noTestHosts", () => {
	it("refuses every call in words", async () => {
		const refused = {
			status: "ok",
			data: { status: "refused", reason: TEST_HOSTS_NOT_SET_UP },
		};
		expect(await noTestHosts.checkTestHosts(MAC)).toEqual(refused);
		expect(await noTestHosts.buildTestHost(MAC, MAC_MACOS)).toEqual(refused);
		expect(await noTestHosts.resetTestHost(MAC, MAC_IOS)).toEqual(refused);
		expect(await noTestHosts.endLease(MAC, IOS_LEASE)).toEqual(refused);
		expect(await noTestHosts.readLeaseScreen(MAC, IOS_LEASE)).toEqual(refused);
	});
});

describe("mockTestHosts", () => {
	it("starts with the Mac's phones and VM and devbox's Linux, one lease held", () => {
		const { hostOf, leases } = setup();
		expect(hostOf(MAC_IOS)).toMatchObject({ device: MAC, backend: "mac_vm", state: "leased" });
		expect(hostOf(MAC_ANDROID)).toMatchObject({ device: MAC, state: "warm" });
		expect(hostOf(MAC_MACOS)).toMatchObject({ device: MAC, kind: "macos", state: "off" });
		expect(hostOf(DEVBOX_LINUX)).toMatchObject({ device: DEVBOX, backend: "incus_test" });
		expect(leases().map((l) => [l.id, l.host, l.state])).toEqual([
			[IOS_LEASE, MAC_IOS, "ready"],
		]);
	});

	it("builds a host that's off and refuses one that's built", async () => {
		const { calls, hostOf, changed } = setup();
		expect(await calls.buildTestHost(MAC, MAC_MACOS)).toEqual(DONE);
		expect(hostOf(MAC_MACOS)?.state).toBe("warm");
		expect(changed).toHaveBeenCalledTimes(1);
		expect(await calls.buildTestHost(MAC, MAC_ANDROID)).toMatchObject({
			data: { status: "refused" },
		});
	});

	it("ends a lease and frees its host", async () => {
		const { calls, hostOf, leases } = setup();
		expect(await calls.endLease(MAC, IOS_LEASE)).toEqual(DONE);
		expect(leases()).toEqual([]);
		expect(hostOf(MAC_IOS)).toMatchObject({ state: "warm", last_reset: NOW });
	});

	it("resets a leased host, which ends its lease", async () => {
		const { calls, hostOf, leases } = setup();
		expect(await calls.resetTestHost(MAC, MAC_IOS)).toEqual(DONE);
		expect(leases()).toEqual([]);
		expect(hostOf(MAC_IOS)?.state).toBe("warm");
		expect(await calls.resetTestHost(MAC, MAC_MACOS)).toMatchObject({
			data: { status: "refused" },
		});
	});

	it("shows a ready lease's screen", async () => {
		const { calls } = setup();
		expect(await calls.readLeaseScreen(MAC, IOS_LEASE)).toMatchObject({
			data: { status: "lease_screen", image: { mime: "image/png" }, at: NOW },
		});
	});

	it("detects a kind ticked where there's no host for it yet", () => {
		const { place, hostOf, changed } = setup();
		place(DEVBOX, ["linux", "android_emulator"]);
		expect(hostOf(`host-${DEVBOX}-android_emulator`)).toMatchObject({
			device: DEVBOX,
			kind: "android_emulator",
			backend: "host_emulator",
			state: "unavailable",
			setup: expect.stringMatching(/^The Android SDK isn't installed\./),
		});
		expect(changed).toHaveBeenCalledTimes(1);
		place(DEVBOX, ["linux", "android_emulator"]);
		expect(changed).toHaveBeenCalledTimes(1);
	});

	it("finds nothing on the wrong device", async () => {
		const { calls } = setup();
		expect(await calls.endLease(DEVBOX, IOS_LEASE)).toMatchObject({
			status: "error",
			error: { kind: "not_found" },
		});
		expect(await calls.buildTestHost(DEVBOX, MAC_MACOS)).toMatchObject({ status: "error" });
		expect(await calls.checkTestHosts("dev-gone")).toMatchObject({ status: "error" });
		expect(await calls.checkTestHosts(DEVBOX)).toEqual(DONE);
	});
});
