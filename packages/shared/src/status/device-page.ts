// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The parts of a device's page every client words the same: the chips
// under its name, a failed check split into what failed and why, and the
// windows a wind down offers.

import { t } from "../i18n";

import type { DeviceNode } from "../view/select";

/** Each operating system's name, by the name Rust gives it. */
export const OS_NAME: Readonly<Record<string, string>> = {
	linux: "Linux",
	macos: "macOS",
	windows: "Windows",
	freebsd: "FreeBSD",
};

/** The chips under the device's name. */
export const deviceChips = (node: DeviceNode): string[] => {
	const { link, device, network } = node.ctx;
	const chips: string[] = [];
	if (network?.kind.type === "tailscale") chips.push(t("Needs Tailscale"));
	chips.push(
		link.route.type === "ssh" ? t("SSH as {{host}}", { host: link.route.host }) : t("This Mac")
	);
	if (device) {
		chips.push(OS_NAME[device.os] ?? device.os);
		chips.push(
			node.look === "live"
				? t("maxd {{maxd_version}}", { maxd_version: device.maxd_version })
				: t("maxd {{maxd_version}} when last seen", { maxd_version: device.maxd_version })
		);
	}
	return chips;
};

/** A failed check's detail split into the finding and the reason behind it. */
export const splitDetail = (detail: string): [string, string | undefined] => {
	const at = detail.indexOf(". ");
	return at < 0 ? [detail, undefined] : [detail.slice(0, at), detail.slice(at + 2)];
};

/**
 * How long busy agents get to finish before a restart, in minutes. `maxd
 * host quiesce` gives 20 unless told otherwise.
 */
export const WIND_DOWN_MINUTES: readonly number[] = [2, 5, 10, 20, 30, 60];
export const DEFAULT_WIND_DOWN_MINUTES = 20;
