// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the views read about test hosts: whether a device offers them, what
// it hosts and how its settings stand, and the leases agents and keys hold.

import { t } from "../i18n";

import type {
	Actor,
	DeviceId,
	TestHost,
	TestHostBackend,
	TestHostId,
	TestHostSettings,
	TestKind,
	TestLease,
	Timestamp,
} from "../bindings";
import { all, type ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

/** Every kind, in the order the views list them, as `TestKind::ALL` orders them. */
export const TEST_KINDS: readonly TestKind[] = [
	"ios_simulator",
	"android_emulator",
	"linux",
	"macos",
];

/** What a kind is called everywhere. */
export const kindLabel = (kind: TestKind): string =>
	({
		ios_simulator: "iOS Simulator",
		android_emulator: t("Android emulator"),
		linux: t("Linux"),
		macos: "macOS VM",
	})[kind];

/** What a backend is called everywhere. */
export const backendLabel = (backend: TestHostBackend): string =>
	({
		incus_test: t("Test container"),
		mac_vm: t("Mac VM"),
		mac_host: t("This Mac"),
		host_emulator: t("Emulator"),
	})[backend];

/** A device's test host settings before you change any, as `maxd` defaults them. */
export const NO_TEST_HOSTS: Required<TestHostSettings> = {
	kinds: [],
	mac_host: false,
	max_vms: 1,
	default_minutes: 30,
	longest_minutes: 240,
	idle_minutes: 10,
};

/** Whether `device`'s `maxd` takes test host requests. */
export const offersTestHosts = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.test_hosts === true;

/** `device`'s test host settings, with every field filled. */
export const testHostSettingsOf = (
	view: ViewState,
	device: DeviceId
): Required<TestHostSettings> => ({
	...NO_TEST_HOSTS,
	...view.tables.device_settings.get(device)?.test_hosts,
});

/** Whether two settings say the same, ignoring the order of their kinds. */
export const sameTestHostSettings = (a: TestHostSettings, b: TestHostSettings): boolean => {
	const x = { ...NO_TEST_HOSTS, ...a };
	const y = { ...NO_TEST_HOSTS, ...b };
	return (
		x.kinds.length === y.kinds.length &&
		x.kinds.every((kind) => y.kinds.includes(kind)) &&
		x.mac_host === y.mac_host &&
		x.max_vms === y.max_vms &&
		x.default_minutes === y.default_minutes &&
		x.longest_minutes === y.longest_minutes &&
		x.idle_minutes === y.idle_minutes
	);
};

/**
 * Whether a device with this OS, as `Device.os` names it, could host a kind.
 * Linux runs in Incus on Linux, the Android emulator on either, and the
 * rest on a Mac.
 */
export const couldHost = (os: string, kind: TestKind): boolean => {
	if (kind === "android_emulator") return os === "linux" || os === "macos";
	return os === (kind === "linux" ? "linux" : "macos");
};

const byKind = (a: { kind: TestKind }, b: { kind: TestKind }) =>
	TEST_KINDS.indexOf(a.kind) - TEST_KINDS.indexOf(b.kind);

/** The test hosts `device` publishes, in kind order. */
export const testHostsOn = (view: ViewState, device: DeviceId): TestHost[] =>
	all(view, "test_host")
		.filter((host) => host.device === device)
		.sort(byKind);

/** The leases `device` grants that haven't ended, the oldest first. */
export const leasesOn = (view: ViewState, device: DeviceId): TestLease[] =>
	all(view, "test_lease")
		.filter((lease) => lease.device === device && lease.state !== "ended")
		.sort((a, b) => a.started_at - b.started_at);

const sameActor = (a: Actor, b: Actor): boolean =>
	(a.kind === "agent" && b.kind === "agent" && a.agent === b.agent) ||
	(a.kind === "key" && b.kind === "key" && a.key === b.key) ||
	(a.kind === "task" && b.kind === "task" && a.task === b.task);

/** The leases `holder` holds on every device that haven't ended, the oldest first. */
export const leasesOf = (view: ViewState, holder: Actor): TestLease[] =>
	all(view, "test_lease")
		.filter((lease) => lease.state !== "ended" && sameActor(lease.holder, holder))
		.sort((a, b) => a.started_at - b.started_at);

/** The lease on a host now, if one holds it. */
export const leaseOfHost = (view: ViewState, host: TestHostId): TestLease | undefined =>
	all(view, "test_lease").find(
		(lease) => lease.host === host && lease.state !== "queued" && lease.state !== "ended"
	);

/** Whole minutes until a lease ends, by its time or by sitting idle, whichever comes first. */
export const minutesLeft = (lease: TestLease, now: Timestamp): number =>
	Math.max(0, Math.ceil((Math.min(lease.ends_at, lease.idle_ends_at) - now) / 60_000));
