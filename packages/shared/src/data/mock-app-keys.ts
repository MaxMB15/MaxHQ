// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The app keys of the sample data: the Mac and devbox both trust this Mac's
// key, and devbox also trusts a key from another Mac that this one didn't
// add. A new key moves to each device at once. They live in memory.

import type { AppKey, AppKeyState, DeviceId } from "../bindings";
import type { Entity } from "../fleet/wire";
import { DEVBOX, MAC } from "./mock-view";
import type { DataSource } from "./source";

const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;

/** This Mac's sample key, as base64url of 32 bytes. */
export const MOCK_APP_KEY = "c3R1ZGlvLW1hYy1hcHAta2V5LWZvci1zYW1wbGVzLTAx";

/** Another Mac's key that devbox trusts. */
const OTHER_KEY = "b3RoZXItbWFjLWFwcC1rZXktZm9yLXRoZS1zYW1wbGVz";

/** This Mac's key once the sample makes a new one. */
const NEW_KEY = "c3R1ZGlvLW1hYy1hcHAta2V5LWZvci1zYW1wbGVzLTAy";

const LABEL = "MaxHQ on studio-mac";

/** The sample app keys `device` trusts, as its maxd publishes them. */
const key = (
	device: DeviceId,
	public_key: string,
	fields: Pick<AppKey, "label" | "origin" | "added_at" | "proved_at">
): Entity => ({
	entity: "app_key",
	id: `app-key-${device}-${public_key.slice(-4)}`,
	device,
	public_key,
	...fields,
});

/** The app keys the sample devices trust, as of `now`. */
export const mockAppKeyEntities = (now: number): Entity[] => [
	key(MAC, MOCK_APP_KEY, {
		label: LABEL,
		origin: "signature",
		added_at: now - 30 * DAY,
		proved_at: now - HOUR,
	}),
	key(DEVBOX, MOCK_APP_KEY, {
		label: LABEL,
		origin: "setup",
		added_at: now - 30 * DAY,
		proved_at: now - HOUR,
	}),
	key(DEVBOX, OTHER_KEY, {
		label: "MaxHQ on travel-mac",
		origin: "first",
		added_at: now - 2 * DAY,
		proved_at: null,
	}),
];

/** The sample entities the app key calls read and change. */
export type MockAppKeyStore = {
	/** Every app key entity, on every device. */
	readonly keys: () => Entity[];
	/** Replaces every app key entity. */
	readonly set: (keys: Entity[]) => void;
};

/** The app key calls over the sample devices. */
export const mockAppKeys = (
	now: () => number,
	store: MockAppKeyStore
): Pick<DataSource, "appKey" | "removeAppKey" | "rotateAppKey"> => {
	let own = MOCK_APP_KEY;
	return {
		appKey: async () => ({
			status: "ok",
			data: {
				public_key: own,
				previous: null,
				waiting: [],
				refusals: [],
				remade: null,
				kept: true,
			} satisfies AppKeyState,
		}),
		removeAppKey: async (device, publicKey) => {
			if (publicKey === own) {
				return {
					status: "error",
					error: {
						kind: "failed",
						message: "This Mac's own app key can't be removed here",
					},
				};
			}
			store.set(
				store
					.keys()
					.filter(
						(e) =>
							!(
								e.entity === "app_key" &&
								e.device === device &&
								e.public_key === publicKey
							)
					)
			);
			return { status: "ok", data: { status: "done" } };
		},
		rotateAppKey: async () => {
			const old = own;
			own = own === NEW_KEY ? MOCK_APP_KEY : NEW_KEY;
			store.set(
				store.keys().flatMap((e) => {
					if (e.entity !== "app_key" || e.public_key !== old) return [e];
					return [
						key(e.device, own, {
							label: LABEL,
							origin: "app",
							added_at: now(),
							proved_at: now(),
						}),
					];
				})
			);
			return { status: "ok", data: null };
		},
	};
};
