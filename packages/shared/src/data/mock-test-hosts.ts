// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The test host calls of the sample data and of tests. The Mac hosts the
// iOS Simulator in a VM, leased by the signed bundle agent, the Android
// emulator on itself, and a macOS VM that's off. devbox hosts Linux in a
// test container. Ticking a kind where the sample has no host for it
// finds what the device lacks, the way detection would. Changes live in
// memory, and each host and lease goes out as its own entity the way
// `maxd` publishes them.

import type {
	DeviceId,
	Entity,
	LeaseId,
	TestHost,
	TestHostBackend,
	TestHostId,
	TestHostSettings,
	TestKind,
	TestLease,
	Timestamp,
	VmSize,
} from "../bindings";
import { BUNDLE } from "./mock-started";
import { DEVBOX, MAC } from "./mock-view";
import type { ActionResult, DataSource } from "./source";
import { testHostsNotSetUp } from "./test-hosts";

/** The test host calls of a data source. */
export type TestHostCalls = Pick<
	DataSource,
	"checkTestHosts" | "buildTestHost" | "resetTestHost" | "endLease" | "readLeaseScreen"
>;

/** The test host calls with no test hosts behind them, refused the way `maxd` does for now. */
export const noTestHosts: TestHostCalls = {
	checkTestHosts: testHostsNotSetUp,
	buildTestHost: testHostsNotSetUp,
	resetTestHost: testHostsNotSetUp,
	endLease: testHostsNotSetUp,
	readLeaseScreen: testHostsNotSetUp,
};

/** The Mac's iOS Simulator VM. */
export const MAC_IOS = "host-mac-ios-vm";
/** The Android emulator on the Mac itself. */
export const MAC_ANDROID = "host-mac-android";
/** The Mac's macOS VM, off until you build it. */
export const MAC_MACOS = "host-mac-macos-vm";
/** devbox's Linux test container. */
export const DEVBOX_LINUX = "host-devbox-linux";
/** The signed bundle agent's lease of the iOS Simulator. */
export const IOS_LEASE = "lease-ios";

/** What each sample device hosts, as its settings say. */
export const MOCK_TEST_HOST_SETTINGS: Record<DeviceId, TestHostSettings> = {
	[MAC]: {
		kinds: ["ios_simulator", "android_emulator", "macos"],
		mac_host: false,
		max_vms: 1,
		default_minutes: 30,
		longest_minutes: 240,
		idle_minutes: 10,
	},
	[DEVBOX]: {
		kinds: ["linux"],
		mac_host: false,
		max_vms: 1,
		default_minutes: 30,
		longest_minutes: 240,
		idle_minutes: 10,
	},
};

const MIN = 60_000;
const GB = 1024 ** 3;

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

/** A Mac VM's base disk and memory. */
const VM: VmSize = { disk_bytes: 60 * GB, memory_bytes: 8 * GB };

/** A 1 by 1 PNG, standing in for a screenshot. */
const SCREEN_PNG =
	"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

const host = (
	over: Partial<TestHost> & Pick<TestHost, "id" | "device" | "kind" | "backend" | "state">
): TestHost => ({
	setup: null,
	error: null,
	agent_device: null,
	last_reset: null,
	queue: 0,
	vm: null,
	...over,
});

const sampleHosts = (now: Timestamp): TestHost[] => [
	host({
		id: MAC_IOS,
		device: MAC,
		kind: "ios_simulator",
		backend: "mac_vm",
		state: "leased",
		agent_device: "iPhone 16",
		last_reset: now - 40 * MIN,
		vm: VM,
	}),
	host({
		id: MAC_ANDROID,
		device: MAC,
		kind: "android_emulator",
		backend: "host_emulator",
		state: "warm",
		agent_device: "Pixel 9, API 35",
		last_reset: now - 3 * 60 * MIN,
	}),
	host({ id: MAC_MACOS, device: MAC, kind: "macos", backend: "mac_vm", state: "off", vm: VM }),
	host({
		id: DEVBOX_LINUX,
		device: DEVBOX,
		kind: "linux",
		backend: "incus_test",
		state: "warm",
		last_reset: now - 90 * MIN,
	}),
];

const sampleLeases = (now: Timestamp): TestLease[] => [
	{
		id: IOS_LEASE,
		device: MAC,
		host: MAC_IOS,
		kind: "ios_simulator",
		holder: { kind: "agent", agent: BUNDLE },
		permission: "perm-release",
		started_at: now - 12 * MIN,
		ends_at: now + 18 * MIN,
		idle_ends_at: now + 8 * MIN,
		state: "ready",
		target: "00000000-0000-0000-0000-000000000001",
		ended: null,
		recent: [
			{
				at: now - 10 * MIN,
				command: ["xcrun", "simctl", "install", "booted", "MaxHQ.app"],
				exit_code: 0,
			},
			{
				at: now - 2 * MIN,
				command: ["xcrun", "simctl", "launch", "booted", "com.example.maxhq"],
				exit_code: 0,
			},
		],
	},
];

// What detection finds for a kind ticked on a device with no host for it.
const DETECTED: Record<TestKind, { backend: TestHostBackend; setup: string }> = {
	android_emulator: {
		backend: "host_emulator",
		setup: "The Android SDK isn't installed. Install Android Studio or its command line tools, then check again. MaxHQ will install it for you in a later version.",
	},
	linux: {
		backend: "incus_test",
		setup: "Incus isn't installed. Install it as for container workspaces, then check again.",
	},
	ios_simulator: {
		backend: "mac_vm",
		setup: "iOS Simulators need macOS with Xcode.",
	},
	macos: {
		backend: "mac_vm",
		setup: "A macOS VM needs a Mac with Apple silicon.",
	},
};

/** The sample test hosts, as calls and the entities that show them. */
export type MockTestHosts = {
	readonly calls: TestHostCalls;
	/** Detects each of `kinds` on `device` that has no host there yet. */
	readonly place: (device: DeviceId, kinds: readonly TestKind[]) => void;
	/** Each test host and lease entity as it stands. */
	readonly entities: () => Entity[];
};

/**
 * The sample test hosts and leases, kept in memory. `changed` runs after
 * each change, so the caller can publish the new entities.
 */
export const mockTestHosts = ({
	now,
	changed = () => {},
}: {
	now: () => Timestamp;
	changed?: () => void;
}): MockTestHosts => {
	const hosts = new Map<TestHostId, TestHost>(sampleHosts(now()).map((h) => [h.id, h]));
	const leases = new Map<LeaseId, TestLease>(sampleLeases(now()).map((l) => [l.id, l]));

	const missing = (what: string): ActionResult => ({
		status: "error",
		error: { kind: "not_found", what },
	});
	const refused = (reason: string): ActionResult => ({
		status: "ok",
		data: { status: "refused", reason },
	});

	/** Puts `id` back to warm and clean, ending any lease on it. */
	const clean = (id: TestHostId) => {
		for (const lease of leases.values()) if (lease.host === id) leases.delete(lease.id);
		hosts.set(id, { ...hosts.get(id)!, state: "warm", error: null, last_reset: now() });
		changed();
	};

	const hostOn = (device: DeviceId, id: TestHostId) => {
		const found = hosts.get(id);
		return found?.device === device ? found : undefined;
	};
	const leaseOn = (device: DeviceId, id: LeaseId) => {
		const found = leases.get(id);
		return found?.device === device ? found : undefined;
	};

	const calls: TestHostCalls = {
		checkTestHosts: async (device) =>
			device === MAC || device === DEVBOX ? DONE : missing(`device ${device}`),
		buildTestHost: async (device, id) => {
			const found = hostOn(device, id);
			if (!found) return missing(`test host ${id}`);
			if (found.state !== "off" && found.state !== "failed") {
				return refused("Only a test host that's off or failed can be built.");
			}
			clean(id);
			return DONE;
		},
		resetTestHost: async (device, id) => {
			const found = hostOn(device, id);
			if (!found) return missing(`test host ${id}`);
			if (found.state === "off" || found.state === "unavailable") {
				return refused("Build this test host first.");
			}
			clean(id);
			return DONE;
		},
		endLease: async (device, id) => {
			const found = leaseOn(device, id);
			if (!found) return missing(`lease ${id}`);
			clean(found.host);
			return DONE;
		},
		readLeaseScreen: async (device, id) => {
			const found = leaseOn(device, id);
			if (!found) return missing(`lease ${id}`);
			if (found.state !== "ready") return refused("This lease isn't ready yet.");
			return {
				status: "ok",
				data: {
					status: "lease_screen",
					image: { mime: "image/png", data: SCREEN_PNG },
					at: now(),
				},
			};
		},
	};

	const place = (device: DeviceId, kinds: readonly TestKind[]) => {
		const missingKinds = kinds.filter(
			(kind) => ![...hosts.values()].some((h) => h.device === device && h.kind === kind)
		);
		for (const kind of missingKinds) {
			const id = `host-${device}-${kind}`;
			hosts.set(id, host({ id, device, kind, state: "unavailable", ...DETECTED[kind] }));
		}
		if (missingKinds.length > 0) changed();
	};

	return {
		calls,
		place,
		entities: () => [
			...[...hosts.values()].map((h) => ({ entity: "test_host" as const, ...h })),
			...[...leases.values()].map((l) => ({ entity: "test_lease" as const, ...l })),
		],
	};
};
