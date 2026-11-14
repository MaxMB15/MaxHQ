// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { TestHost, TestLease } from "../bindings";
import { mockTestHosts } from "../data/mock-test-hosts";
import { MIN, NOW } from "../test/build";

import {
	hostActions,
	hostLine,
	leaseChip,
	leaseCommandLine,
	leaseLine,
	leaseScreenOf,
	vmLine,
} from "./format";

const sample = mockTestHosts({ now: () => NOW }).entities();
const [lease] = sample.filter((e) => e.entity === "test_lease") as unknown as TestLease[];

const host = (over: Partial<TestHost>): TestHost => ({
	id: "h",
	device: "dev-mac",
	kind: "android_emulator",
	backend: "host_emulator",
	state: "warm",
	setup: null,
	error: null,
	agent_device: null,
	last_reset: null,
	queue: 0,
	vm: null,
	...over,
});

describe("test host words", () => {
	it("says where a host stands", () => {
		expect(hostLine(host({ state: "unavailable", setup: "Install Xcode." }), NOW)).toBe(
			"Install Xcode."
		);
		expect(hostLine(host({ state: "off" }), NOW)).toMatch(/Build it to start leasing\.$/);
		expect(hostLine(host({ last_reset: NOW - 5 * MIN }), NOW)).toBe(
			"Ready for a lease, clean since 14:43."
		);
		expect(hostLine(host({}), NOW)).toBe("Ready for a lease.");
		expect(hostLine(host({ state: "leased", queue: 2 }), NOW)).toBe(
			"An agent holds it, and 2 more requests wait."
		);
		expect(hostLine(host({ state: "failed", error: "The disk is full." }), NOW)).toBe(
			"The disk is full."
		);
	});

	it("offers Build for a host that's off or failed and Reset once it's built", () => {
		expect(hostActions(host({ state: "off" }))).toEqual({ build: true, reset: false });
		expect(hostActions(host({ state: "failed" }))).toEqual({ build: true, reset: true });
		expect(hostActions(host({ state: "leased" }))).toEqual({ build: false, reset: true });
		expect(hostActions(host({ state: "unavailable" }))).toEqual({ build: false, reset: false });
		expect(hostActions(host({ state: "building" }))).toEqual({ build: false, reset: false });
	});

	it("gives a VM's size in GB", () => {
		expect(vmLine({ disk_bytes: 60 * 1024 ** 3, memory_bytes: 8 * 1024 ** 3 })).toBe(
			"60 GB disk, 8 GB memory"
		);
	});

	it("says how a lease stands and how long it has", () => {
		expect(leaseLine(lease!, NOW)).toBe("Ready, 8 min left");
		expect(leaseChip(lease!, NOW)).toBe("iOS Simulator, 8 min");
		expect(leaseLine({ ...lease!, state: "queued" }, NOW)).toBe("Waiting for the host");
		expect(leaseChip({ ...lease!, state: "queued" }, NOW)).toBe("iOS Simulator, waiting");
		expect(leaseLine({ ...lease!, state: "ended", ended: "idle" }, NOW)).toBe(
			"It went too long without a command."
		);
	});

	it("writes a command as a shell line", () => {
		expect(leaseCommandLine({ at: NOW, command: ["open", "My App.app"], exit_code: 0 })).toBe(
			"open 'My App.app'"
		);
	});

	it("reads a screenshot out of its answer", async () => {
		const answer = await mockTestHosts({ now: () => NOW }).calls.readLeaseScreen(
			"dev-mac",
			lease!.id
		);
		expect(leaseScreenOf(answer)).toEqual({
			src: expect.stringMatching(/^data:image\/png;base64,iVBOR/),
			at: NOW,
		});
		expect(leaseScreenOf({ status: "ok", data: { status: "done" } })).toBeNull();
	});
});
