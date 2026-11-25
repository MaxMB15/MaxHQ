// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A WSL distro is a device of its own, reached through `wsl.exe` on the
// Windows machine it runs on: this one, or another one over SSH. These
// say which, in the same words everywhere the route shows.

import { t } from "../i18n";

import { shellQuote } from "../agent-tools/scope";
import type { DeviceLink, Route } from "../bindings";

/** A route to a WSL distro. */
export type WslRoute = Extract<Route, { type: "wsl" }>;

/**
 * The name of the Windows machine a WSL route's distro is on: the device a
 * link in `links` reaches at the route's SSH destination, else that
 * destination. Null when it's this machine.
 */
export const wslMachine = (links: Iterable<DeviceLink>, route: WslRoute): string | null => {
	if (route.via === null) return null;
	const at = (r: Route) => r.type === "ssh" && r.host === route.via;
	for (const link of links)
		if (at(link.route) || link.routes?.some((choice) => at(choice.route))) return link.name;
	return route.via;
};

/**
 * A WSL distro in words, such as "WSL Ubuntu on this PC" or "WSL Ubuntu on
 * Desk PC". `machine` is the Windows machine's name, null for this one,
 * and its SSH destination when left out.
 */
export const wslLabel = (route: WslRoute, machine: string | null = route.via): string =>
	machine === null
		? t("WSL {{distro}} on this PC", { distro: route.distro })
		: t("WSL {{distro}} on {{machine}}", { distro: route.distro, machine: machine });

/**
 * The command that opens a shell in a WSL route's distro from a terminal
 * here: `wsl.exe` on this PC, or through `ssh -t` on another.
 */
export const wslCommand = (route: WslRoute): string => {
	const line = `wsl.exe -d ${route.distro}`;
	return route.via === null ? line : `ssh -t ${shellQuote(route.via)} ${line}`;
};
