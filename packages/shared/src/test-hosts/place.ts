// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where each kind of test machine runs, as you place it: per kind, the
// devices that could host it, whether you ticked each, a hint where one
// suits it best, and the hosts its `maxd` built or detected for it.

import { t } from "../i18n";

import type {
	Device,
	DeviceId,
	TestHost,
	TestHostSettings,
	TestKind,
	Timestamp,
} from "../bindings";
import { deviceConnection } from "../status/connection";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkContext, linkForDevice } from "../view/select";
import { couldHost, offersTestHosts, TEST_KINDS, testHostSettingsOf, testHostsOn } from "./hosts";

/** One device that could host one kind. */
export type Place = {
	readonly kind: TestKind;
	readonly device: DeviceId;
	readonly name: string;
	/** Whether you picked it to host the kind. */
	readonly ticked: boolean;
	/** Why it suits the kind best, shown beside the tick, or null. */
	readonly suggested: string | null;
	/** Its hosts of the kind, as its `maxd` publishes them. */
	readonly hosts: readonly TestHost[];
};

/** One kind and the devices that could host it. */
export type KindPlaces = { readonly kind: TestKind; readonly places: readonly Place[] };

/** A built host is one past its build. */
const built = (host: TestHost): boolean =>
	host.state !== "unavailable" && host.state !== "off" && host.state !== "building";

/**
 * Why a device suits a kind best: a Mac runs the Android emulator better
 * than a server does, and an iOS Simulator in a built VM keeps the app
 * under test away from your files. A hint only, never a tick.
 */
export const suggestion = (
	kind: TestKind,
	os: string,
	hosts: readonly TestHost[]
): string | null => {
	if (kind === "android_emulator" && os === "macos")
		return t("Suggested, runs the emulator best");
	if (kind === "ios_simulator" && hosts.some((h) => h.backend === "mac_vm" && built(h))) {
		return t("Suggested, its VM is built");
	}
	return null;
};

const placeOf = (view: ViewState, device: Device, kind: TestKind): Place => {
	const hosts = testHostsOn(view, device.id).filter((h) => h.kind === kind);
	return {
		kind,
		device: device.id,
		name: deviceName(view, device.id),
		ticked: testHostSettingsOf(view, device.id).kinds.includes(kind),
		suggested: suggestion(kind, device.os, hosts),
		hosts,
	};
};

/** The devices whose `maxd` offers test hosts, by the name you gave each. */
export const testHostDevices = (view: ViewState): Device[] =>
	all(view, "device")
		.filter((d) => offersTestHosts(view, d.id))
		.sort((a, b) => deviceName(view, a.id).localeCompare(deviceName(view, b.id)));

/** Every kind, each with the devices that offer test hosts and could host it. */
export const kindPlaces = (view: ViewState): KindPlaces[] => {
	const devices = testHostDevices(view);
	return TEST_KINDS.map((kind) => ({
		kind,
		places: devices.filter((d) => couldHost(d.os, kind)).map((d) => placeOf(view, d, kind)),
	}));
};

/** The kinds one device could host, for its detail. None when it doesn't offer test hosts. */
export const placesOn = (view: ViewState, device: DeviceId): Place[] => {
	const found = view.tables.device.get(device);
	if (!found || !offersTestHosts(view, device)) return [];
	return TEST_KINDS.filter((kind) => couldHost(found.os, kind)).map((kind) =>
		placeOf(view, found, kind)
	);
};

/** The settings with `kind` ticked or not, the kinds kept in the views' order. */
export const withKind = (
	settings: TestHostSettings,
	kind: TestKind,
	on: boolean
): TestHostSettings => {
	const kinds = (settings.kinds ?? []).filter((k) => k !== kind);
	if (on) kinds.push(kind);
	return { ...settings, kinds: TEST_KINDS.filter((k) => kinds.includes(k)) };
};

/** Whether `device`'s `maxd` answers now, so it can take a change. */
export const deviceLive = (view: ViewState, device: DeviceId, now: Timestamp): boolean => {
	const link = linkForDevice(view, device);
	return link !== undefined && deviceConnection(linkContext(view, link), now).look === "live";
};
