// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which Mac's dispatcher a client talks to, the account it runs as there,
// and why it can't take a request now. The desktop talks to the Mac it runs
// on, the phone to the Mac it's paired with. Only where a client keeps the
// account you picked differs between them.

import { t } from "../i18n";

import type { Account, AccountId, DeviceId, DeviceLink, Dispatcher } from "../bindings";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";

import { dispatcherAccount } from "./plan";

const connected = (link: DeviceLink): boolean => link.state.state === "connected";

// The first link's device, a connected one first.
const firstOf = (links: readonly DeviceLink[]): DeviceId | null =>
	(links.find(connected) ?? links[0])?.device ?? null;

/**
 * The device whose dispatcher this client talks to: the one it runs on,
 * else one that has a dispatcher, else a Mac it links to. A connected one
 * comes first.
 */
export const dispatcherDevice = (view: ViewState): DeviceId | null => {
	const links = all(view, "device_link");
	const here = links.find((l) => l.route.type === "local");
	if (here) return here.device;
	const has = (test: (device: DeviceId) => boolean) =>
		links.filter((l) => l.device !== null && test(l.device));
	return (
		firstOf(has((d) => view.tables.dispatcher.has(d))) ??
		firstOf(has((d) => view.tables.device.get(d)?.os === "macos"))
	);
};

/** The dispatcher this client talks to, and whether it can take a request now. */
export type DispatcherReach = {
	readonly device: DeviceId | null;
	/** What the device reports of it. A device that's too old reports none. */
	readonly dispatcher: Dispatcher | undefined;
	/** The account it runs as, or undefined when the device has none it can use. */
	readonly account: Account | undefined;
	/** Why it can't take a request now, in a few words, or null when it can. */
	readonly off: string | null;
};

const offReason = (
	view: ViewState,
	device: DeviceId | null,
	dispatcher: Dispatcher | undefined,
	account: Account | undefined
): string | null => {
	if (!device) return t("Pair a Mac to use the dispatcher");
	const link = linkForDevice(view, device);
	const name = link?.route.type === "local" ? t("this Mac") : deviceName(view, device);
	// A link that dropped keeps the dispatcher it last reported.
	if (link && !connected(link)) return t("Can't reach {{name}}", { name: name });
	if (!dispatcher) return t("Update MaxHQ on {{name}} to use the dispatcher", { name: name });
	if (!account) return t("Sign in to Claude or Codex on {{name}}", { name: name });
	return null;
};

/** The dispatcher, the account it runs as with `picked` while its device has it, and why it's off. */
export const dispatcherReach = (view: ViewState, picked: AccountId | null): DispatcherReach => {
	const device = dispatcherDevice(view);
	const dispatcher = device ? view.tables.dispatcher.get(device) : undefined;
	const account = dispatcherAccount(view, device, picked);
	return { device, dispatcher, account, off: offReason(view, device, dispatcher, account) };
};
