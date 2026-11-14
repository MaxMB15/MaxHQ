// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a device's settings say about the app keys it trusts: each key with
// how and when the device came to trust it, which one is this Mac's, and
// the warnings when the device trusts a key this Mac didn't add or turned
// this Mac's key down. A device that took the first key offered says so,
// with how to start over. Making a new key lists the devices still on the
// old.

import { t } from "../i18n";

import type { AppKey, AppKeyOrigin, AppKeyState, DeviceId, Timestamp } from "../bindings";
import { formatAgo, formatWhen } from "../status/format";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";

/** How a device came to trust a key, in words. */
export const ORIGINS: Record<AppKeyOrigin, string> = {
	get signature() {
		return t("Signature checked");
	},
	get setup() {
		return t("Sent at setup");
	},
	get first() {
		return t("First key offered");
	},
	get approved() {
		return t("Allowed from a card");
	},
	get app() {
		return t("Added by a trusted app");
	},
	get unknown() {
		return t("Added another way");
	},
};

/** Whether `device`'s maxd told of any app key it trusts. One from before app keys tells none. */
export const listsAppKeys = (view: ViewState, device: DeviceId): boolean =>
	all(view, "app_key").some((key) => key.device === device);

/** Whether `device` is this Mac, where a new app key starts. */
export const isThisMac = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.route.type === "local";

/** One key on a device's list. */
export type AppKeyRow = {
	readonly key: AppKey;
	/** Whether it's this Mac's key, now or before a new one. */
	readonly own: boolean;
	/** What the row says under the key's label. */
	readonly sub: string;
};

/** Whether `key` is this Mac's, now or the one before. */
const ownKey = (state: AppKeyState | null, key: string): boolean =>
	state !== null && (key === state.public_key || key === state.previous);

/** The keys `device` trusts, this Mac's first, then the oldest. */
export const appKeyRows = (
	view: ViewState,
	device: DeviceId,
	state: AppKeyState | null,
	now: Timestamp
): AppKeyRow[] =>
	all(view, "app_key")
		.filter((key) => key.device === device)
		.map((key) => {
			const own = ownKey(state, key.public_key);
			const proved =
				key.proved_at === null
					? t("never proved")
					: t("proved {{value1}}", { value1: formatAgo(key.proved_at, now) });
			const mark =
				state === null
					? []
					: key.public_key === state.public_key
						? [t("This Mac")]
						: own
							? [t("This Mac's key before")]
							: [];
			return {
				key,
				own,
				sub: [
					...mark,
					ORIGINS[key.origin],
					t("added {{value1}}", { value1: formatWhen(key.added_at, now) }),
					proved,
				].join(", "),
			};
		})
		.sort(
			(a, b) =>
				Number(b.own) - Number(a.own) ||
				a.key.added_at - b.key.added_at ||
				a.key.public_key.localeCompare(b.key.public_key)
		);

/**
 * The warning when `device` trusts a key this Mac didn't add, from the
 * first time it did. Without this Mac's key nothing can tell.
 */
export const foreignKeyWarning = (
	view: ViewState,
	device: DeviceId,
	state: AppKeyState | null,
	now: Timestamp
): string | null => {
	if (state === null) return null;
	const since = all(view, "app_key")
		.filter((key) => key.device === device && !ownKey(state, key.public_key))
		.reduce<Timestamp | null>(
			(first, key) => (first === null ? key.added_at : Math.min(first, key.added_at)),
			null
		);
	if (since === null) return null;
	return t(
		"{{deviceName_view__device}} has trusted an app key this Mac didn't add since {{formatWhen_since__now}}.",
		{
			deviceName_view__device: deviceName(view, device),
			formatWhen_since__now: formatWhen(since, now),
		}
	);
};

/**
 * What to say when `device` turned this Mac's key down: that it no longer
 * trusts it when it did before, or that it doesn't, with what it said.
 */
export const refusalNote = (
	view: ViewState,
	device: DeviceId,
	state: AppKeyState | null
): string | null => {
	const refusal = state?.refusals.find((r) => r.device === device);
	if (!refusal) return null;
	const name = deviceName(view, device);
	if (refusal.kept_previous) {
		return t(
			"{{name}} turned down this Mac's new app key and still uses the one before: {{refusal_reason}}",
			{ name: name, refusal_reason: refusal.reason }
		);
	}
	return refusal.trusted_before
		? t("{{name}} no longer trusts this Mac's app key: {{refusal_reason}}", {
				name: name,
				refusal_reason: refusal.reason,
			})
		: t("{{name}} doesn't trust this Mac's app key: {{refusal_reason}}", {
				name: name,
				refusal_reason: refusal.reason,
			});
};

/** A note on how a device came to trust the first key it was offered. */
export type FirstKeyNote = {
	readonly text: string;
	/** Whether it may have let another program in, so it reads as a warning. */
	readonly warning: boolean;
};

/** How to make a device forget its app keys and take the next one offered. */
const startOver = (name: string): string =>
	t(
		"To start over, delete phones/app-keys.json in maxd's data folder on {{name}} and restart maxd there. It then trusts the first app that connects.",
		{ name }
	);

/**
 * What to say when `device` took the first app key it was offered without
 * checking the program that offered it, as a remote device or this Mac
 * with an unsigned maxd does: whether that was this Mac's key, and how to
 * start over when it wasn't. A device that never trusted this Mac's key
 * shows this Mac none of its keys, so it gets only how to start over.
 */
export const firstKeyNote = (
	view: ViewState,
	device: DeviceId,
	state: AppKeyState | null
): FirstKeyNote | null => {
	const name = deviceName(view, device);
	const keys = all(view, "app_key").filter((key) => key.device === device);
	const first = keys.find((key) => key.origin === "first");
	if (!first) {
		const refusal = state?.refusals.find((r) => r.device === device);
		if (!refusal || refusal.trusted_before || refusal.kept_previous || keys.length > 0) {
			return null;
		}
		return {
			text: t(
				"If no other Mac of yours uses {{name}}, another program may have offered it a key before this Mac did. {{startOver}}",
				{ name, startOver: startOver(name) }
			),
			warning: true,
		};
	}
	const risk = isThisMac(view, device)
		? t(
				"This Mac's maxd isn't signed, so it trusted the first app key it was offered without checking which program offered it."
			)
		: t(
				"{{name}} trusted the first app key it was offered, without checking which program offered it.",
				{ name }
			);
	if (state === null) return { text: risk, warning: true };
	if (ownKey(state, first.public_key)) {
		return { text: t("{{risk}} That was this Mac's key.", { risk }), warning: false };
	}
	const trustsOurs = keys.some((key) => ownKey(state, key.public_key));
	return {
		text: trustsOurs
			? t("{{risk}} It wasn't this Mac's key. Remove it if it isn't yours.", { risk })
			: t("{{risk}} It wasn't this Mac's key. {{startOver}}", {
					risk,
					startOver: startOver(name),
				}),
		warning: true,
	};
};

/** What the new key part says: where the move stands, or what the button does. */
export const rotationNote = (view: ViewState, state: AppKeyState): string => {
	if (state.previous === null) {
		return t(
			"Makes a new key for this Mac and moves every device to it. The old key works until each device has moved."
		);
	}
	if (state.waiting.length === 0) return t("Every device has moved to the new key.");
	const names = state.waiting.map((d) => deviceName(view, d)).sort((a, b) => a.localeCompare(b));
	return t(
		"Still on the old key: {{names_join}}. A device that's off moves when this Mac next reaches it.",
		{ names_join: names.join(", ") }
	);
};

/** Why this Mac made a new key as it opened, or that the key lasts until you quit. */
export const keyStateNote = (state: AppKeyState): string | null => {
	if (!state.kept) {
		return t("The Keychain didn't keep this Mac's app key, so it lasts until you quit MaxHQ.");
	}
	return state.remade === null
		? null
		: t("This Mac made a new app key: {{state_remade}}", { state_remade: state.remade });
};
