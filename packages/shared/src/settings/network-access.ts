// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a device's Network access settings work from: the port you type,
// whether phones can reach it now, and the phones signed in to it, each
// in one line. The Phones settings take the devices a phone hasn't signed
// in to yet from here too, for a new code.

import { t } from "../i18n";

import type { DeviceId, NetworkAccess, PhoneId, PhoneSession, Timestamp } from "../bindings";
import { formatAgo, formatDuration, formatWhen } from "../status/format";
import { all, type ViewState } from "../view/replica";
import { deviceName } from "../view/select";

import { pairedPhones } from "./phones";
import { parsePorts } from "./project";

/** The port a device listens on until you pick another. */
export const DEFAULT_PORT = 7466;

/** A port typed in, or why it isn't one. */
export const parsePort = (
	text: string
): { ok: true; port: number } | { ok: false; reason: string } => {
	const parsed = parsePorts(text);
	if (!parsed.ok) return parsed;
	const [port, ...more] = parsed.ports;
	if (port === undefined || more.length > 0) {
		return { ok: false, reason: t("Type one port, from 1 to 65535.") };
	}
	return { ok: true, port };
};

/**
 * Whether phones can reach the device over the network now: its switch
 * is on, and it has a pin and an address to give them.
 */
export const networkReady = (access: NetworkAccess): boolean =>
	access.on && access.pin !== null && access.hosts.length > 0;

/** How long a device's one-time code for a phone works, as `CODE_FOR` in the protocol. */
export const CODE_MINUTES = 10;

/**
 * The devices that have `phone` and are ready for it over the network,
 * but have no sign-in from it, by name. A new code from each lets it in.
 */
export const awaitingSignIn = (
	view: ViewState,
	phone: PhoneId
): { readonly device: DeviceId; readonly name: string }[] => {
	const sessions = new Set(
		all(view, "phone_session")
			.filter((session) => session.phone === phone)
			.map((session) => session.device)
	);
	return all(view, "phone")
		.filter((found) => found.id === phone && !sessions.has(found.device))
		.filter((found) => {
			const access = view.tables.network_access.get(found.device);
			return access !== undefined && networkReady(access);
		})
		.map((found) => ({ device: found.device, name: deviceName(view, found.device) }))
		.sort((a, b) => a.name.localeCompare(b.name));
};

/** A phone signed in to a device over its network access. */
export type SignedIn = {
	readonly session: PhoneSession;
	/** The phone's name, as it paired. */
	readonly name: string;
};

/** The phones signed in to `device`, by name. */
export const signedIn = (view: ViewState, device: DeviceId): SignedIn[] => {
	const names = new Map(pairedPhones(view).map((phone) => [phone.id, phone.name]));
	return all(view, "phone_session")
		.filter((session) => session.device === device)
		.map((session) => ({ session, name: names.get(session.phone) ?? t("A phone") }))
		.sort((a, b) => a.name.localeCompare(b.name) || a.session.created - b.session.created);
};

/** Such as "Signed in Thu 09:00, last used 4 min ago, expires in 29 d 23 h". */
export const sessionLine = (session: PhoneSession, now: Timestamp): string => {
	const used =
		session.last_used === null
			? t("never used")
			: t("last used {{formatAgo_session_last_used__now}}", {
					formatAgo_session_last_used__now: formatAgo(session.last_used, now),
				});
	const ends =
		session.expires <= now
			? "expired"
			: t("expires in {{formatDuration_session_expires___now}}", {
					formatDuration_session_expires___now: formatDuration(session.expires - now),
				});
	return t("Signed in {{formatWhen_session_created__now}}, {{used}}, {{ends}}", {
		formatWhen_session_created__now: formatWhen(session.created, now),
		used: used,
		ends: ends,
	});
};
