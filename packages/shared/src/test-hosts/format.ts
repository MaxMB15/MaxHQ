// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The words test hosts and leases show: where a host stands and what you
// can do with it, a VM's size, and a lease's state, time left and the
// commands it ran.

import { t } from "../i18n";

import type {
	LeaseCommand,
	LeaseEnd,
	LeaseState,
	TestHost,
	TestHostState,
	TestLease,
	Timestamp,
	VmSize,
} from "../bindings";
import type { ActionResult } from "../data/source";
import { shellQuote } from "../agent-tools/scope";
import { formatWhen, plural } from "../status/format";
import type { Tone } from "../tasks/format";
import { kindLabel, minutesLeft } from "./hosts";

/** A host's state in a word or two, with its tone. */
export const HOST_STATE: Record<TestHostState, { readonly label: string; readonly tone: Tone }> = {
	unavailable: {
		get label() {
			return t("Needs setup");
		},
		tone: "warn",
	},
	off: {
		get label() {
			return t("Ready to build");
		},
		tone: "plain",
	},
	building: {
		get label() {
			return t("Building");
		},
		tone: "info",
	},
	warm: {
		get label() {
			return t("Warm");
		},
		tone: "good",
	},
	leased: {
		get label() {
			return t("Leased");
		},
		tone: "info",
	},
	resetting: {
		get label() {
			return t("Resetting");
		},
		tone: "info",
	},
	failed: {
		get label() {
			return t("Failed");
		},
		tone: "bad",
	},
};

/** What a host's state means, in a sentence or two. */
export const hostLine = (host: TestHost, now: Timestamp): string => {
	switch (host.state) {
		case "unavailable":
			return host.setup ?? t("This device lacks something the host needs.");
		case "off":
			return t("Everything it needs is here. Build it to start leasing.");
		case "building":
			return t("Making the test machine. This can take a few minutes.");
		case "warm":
			return host.last_reset === null
				? t("Ready for a lease.")
				: t("Ready for a lease, clean since {{formatWhen_host_last_reset__now}}.", {
						formatWhen_host_last_reset__now: formatWhen(host.last_reset, now),
					});
		case "leased":
			return host.queue > 0
				? t("An agent holds it, and {{plural_host_queue___more_request}} wait.", {
						plural_host_queue___more_request: plural(host.queue, "more request"),
					})
				: t("An agent holds it.");
		case "resetting":
			return t("Going back to a clean state after a lease.");
		case "failed":
			return host.error ?? t("The last build or reset failed.");
	}
};

/** What you can do with a host: build one that's off or failed, reset one that's built. */
export const hostActions = (
	host: TestHost
): { readonly build: boolean; readonly reset: boolean } => ({
	build: host.state === "off" || host.state === "failed",
	reset: host.state === "warm" || host.state === "leased" || host.state === "failed",
});

const GIB = 1024 ** 3;

/** A VM's size, such as "60 GB disk, 8 GB memory". */
export const vmLine = (vm: VmSize): string =>
	t("{{value1}} GB disk, {{value2}} GB memory", {
		value1: Math.round(vm.disk_bytes / GIB),
		value2: Math.round(vm.memory_bytes / GIB),
	});

/** A lease's state in a word or two. */
export const LEASE_STATE: Record<LeaseState, string> = {
	get queued() {
		return t("Waiting");
	},
	get starting() {
		return t("Starting");
	},
	get ready() {
		return t("Ready");
	},
	get ending() {
		return t("Ending");
	},
	get ended() {
		return t("Ended");
	},
};

/** Why a lease ended, as a sentence. */
export const LEASE_END: Record<LeaseEnd, string> = {
	get closed() {
		return t("Its holder closed it.");
	},
	get time_up() {
		return t("Its time ran out.");
	},
	get idle() {
		return t("It went too long without a command.");
	},
	get holder_gone() {
		return t("Its holder was archived or deleted.");
	},
	get ended_by_you() {
		return t("You ended it.");
	},
	get host_failed() {
		return t("Its host failed, or maxd restarted while it ran.");
	},
};

/** Where a lease stands and how long it has, such as "Ready, 8 min left". */
export const leaseLine = (lease: TestLease, now: Timestamp): string => {
	if (lease.state === "ended") return lease.ended ? LEASE_END[lease.ended] : t("It ended.");
	if (lease.state === "queued") return t("Waiting for the host");
	return t("{{LEASE_STATE_lease_state}}, {{minutesLeft_lease__now}} min left", {
		LEASE_STATE_lease_state: LEASE_STATE[lease.state],
		minutesLeft_lease__now: minutesLeft(lease, now),
	});
};

/** A lease on an agent's header, such as "iOS Simulator, 8 min". */
export const leaseChip = (lease: TestLease, now: Timestamp): string =>
	lease.state === "queued"
		? t("{{value1}}, waiting", { value1: kindLabel(lease.kind) })
		: t("{{value1}}, {{value2}} min", {
				value1: kindLabel(lease.kind),
				value2: minutesLeft(lease, now),
			});

/** A command a lease ran, as one line for a shell. */
export const leaseCommandLine = (command: LeaseCommand): string =>
	command.command.map(shellQuote).join(" ");

/** A lease's last screenshot, ready for an image. */
export type LeaseScreen = { readonly src: string; readonly at: Timestamp };

/** The screenshot a `ReadLeaseScreen` answered, or null when it answered something else. */
export const leaseScreenOf = (result: ActionResult): LeaseScreen | null =>
	result.status === "ok" && result.data.status === "lease_screen"
		? {
				src: `data:${result.data.image.mime};base64,${result.data.image.data}`,
				at: result.data.at,
			}
		: null;
