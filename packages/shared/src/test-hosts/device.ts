// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a device's page says about its test hosts and leases, on every
// client: the limits it picks from, where a remote agent gets a lease, a
// lease's lines, and the settings a change sends.

import { formatNumber, t } from "../i18n";

import type { DeviceId, LeaseCommand, TestHostSettings, TestLease, Timestamp } from "../bindings";
import type { DeviceSettings } from "../fleet/wire";
import { deviceSettingsOf } from "../settings/device-settings";
import type { ViewState } from "../view/replica";
import { actorName, deviceName } from "../view/select";
import { leaseLine } from "./format";
import { backendLabel, kindLabel } from "./hosts";

/** How often an open lease takes its screen again. */
export const LEASE_SCREEN_EVERY = 2000;

/** Where a remote agent gets a lease, since leases stay on their device. */
export const localNote = (name: string): string =>
	t(
		"Only agents on {{name}} lease its test machines. For an agent elsewhere to test here, start an agent on {{name2}}.",
		{ name: name, name2: name }
	);

/** A limit a device's leases keep, in minutes. */
export type LeaseLimit = "default_minutes" | "longest_minutes" | "idle_minutes";

/** Each limit with its words and the minutes it offers. */
export const LEASE_LIMITS: readonly {
	readonly key: LeaseLimit;
	readonly title: string;
	readonly sub: string;
	readonly choices: readonly number[];
}[] = [
	{
		key: "default_minutes",
		get title() {
			return t("Lease length");
		},
		get sub() {
			return t("How long a lease lasts when the agent doesn't say.");
		},
		choices: [15, 30, 60, 120],
	},
	{
		key: "longest_minutes",
		get title() {
			return t("Longest lease");
		},
		get sub() {
			return t("The most an agent may ask for.");
		},
		choices: [60, 120, 240],
	},
	{
		key: "idle_minutes",
		get title() {
			return t("Ends when idle");
		},
		get sub() {
			return t("A lease ends after this long without a command.");
		},
		choices: [5, 10, 20, 30],
	},
];

/** How many macOS VMs a Mac may run at once: Apple lets it run 2. */
export const VM_CHOICES: readonly number[] = [1, 2];

/** A limit's minutes to pick from, with the one kept even when it's not a choice. */
export const minuteChoices = (
	choices: readonly number[],
	kept: number
): { readonly value: number; readonly label: string }[] =>
	[...new Set([...choices, kept])]
		.sort((a, b) => a - b)
		.map((m) => ({ value: m, label: t("{{m}} min", { m: formatNumber(m) }) }));

/** The settings a save sends for a change to `device`'s test hosts, the rest as it has them. */
export const testHostsChange = (
	view: ViewState,
	device: DeviceId,
	change: (settings: TestHostSettings) => TestHostSettings
): DeviceSettings => {
	const fields = deviceSettingsOf(view, device);
	return { device, ...fields, test_hosts: change(fields.test_hosts) };
};

/** A lease's name, such as "iOS Simulator for Fix the build". */
export const leaseTitle = (view: ViewState, lease: TestLease): string =>
	t("{{value1}} for {{value2}}", {
		value1: kindLabel(lease.kind),
		value2: actorName(view, lease.holder),
	});

/** Where a lease stands and its host, such as "Ready, 8 min left, Mac VM". */
export const leaseSub = (view: ViewState, lease: TestLease, now: Timestamp): string => {
	const host = view.tables.test_host.get(lease.host);
	return [leaseLine(lease, now), host ? backendLabel(host.backend) : null]
		.filter(Boolean)
		.join(", ");
};

/** Who holds a lease, where, and how long it has, as a sentence. */
export const leaseHeld = (view: ViewState, lease: TestLease, now: Timestamp): string => {
	const host = view.tables.test_host.get(lease.host);
	const where = [deviceName(view, lease.device), host ? backendLabel(host.backend) : null]
		.filter(Boolean)
		.join(", ");
	return t("Held by {{value2}} on {{where}}. {{value6}}.", {
		value2: actorName(view, lease.holder),
		where: where,
		value6: leaseLine(lease, now),
	});
};

/** How a command a lease ran ended, such as "Exit 0". */
export const commandOutcome = (command: LeaseCommand): string =>
	command.exit_code === null
		? t("Didn't finish")
		: t("Exit {{exit_code}}", {
				exit_code: formatNumber(command.exit_code, { useGrouping: false }),
			});
