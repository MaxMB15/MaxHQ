// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AppKey, AppKeyState, Entity } from "../bindings";
import { device, entities, link, MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	appKeyRows,
	firstKeyNote,
	foreignKeyWarning,
	keyStateNote,
	refusalNote,
	rotationNote,
} from "./app-keys";

const OWN = "own-key";
const OLD = "old-key";
const OTHER = "other-key";
// Times stay on the same day, so the tests read the same in any time zone.
const HOUR = 60 * MIN;

const key = (over: Partial<AppKey> & { public_key: string }): Entity => ({
	entity: "app_key",
	id: `${over.device ?? "devbox"}:${over.public_key}`,
	device: "devbox",
	label: "MaxHQ on studio-mac",
	added_at: NOW - 3 * HOUR,
	origin: "setup",
	proved_at: NOW - 5 * MIN,
	...over,
});

const state = (over: Partial<AppKeyState> = {}): AppKeyState => ({
	public_key: OWN,
	previous: null,
	waiting: [],
	refusals: [],
	remade: null,
	kept: true,
	...over,
});

const view = (keys: Entity[]) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [
					device({ id: "mac", name: "Mac", os: "macos" }),
					device({ id: "devbox", name: "devbox" }),
					device({ id: "ws-alpha", name: "ws-alpha" }),
				],
				links: [link({ id: "this-mac", device: "mac", route: { type: "local" } })],
			}),
			...keys,
		],
	});

describe("app key rows", () => {
	it("put this Mac's key first and say how and when each came in", () => {
		const v = view([
			key({
				public_key: OTHER,
				label: "MaxHQ on travel-mac",
				origin: "first",
				added_at: NOW - HOUR,
				proved_at: null,
			}),
			key({ public_key: OWN }),
			key({ public_key: OWN, device: "ws-alpha" }),
		]);
		const rows = appKeyRows(v, "devbox", state(), NOW);
		expect(rows.map((r) => [r.key.label, r.own, r.sub])).toEqual([
			["MaxHQ on studio-mac", true, "This Mac, Sent at setup, added 11:48, proved 5 min ago"],
			["MaxHQ on travel-mac", false, "First key offered, added 13:48, never proved"],
		]);
	});

	it("mark the key before a new one as this Mac's too", () => {
		const v = view([key({ public_key: OLD }), key({ public_key: OWN, origin: "app" })]);
		const rows = appKeyRows(v, "devbox", state({ previous: OLD, waiting: ["devbox"] }), NOW);
		expect(rows.every((r) => r.own)).toBe(true);
		expect(rows.map((r) => r.sub.split(", ")[0])).toEqual([
			"This Mac's key before",
			"This Mac",
		]);
	});

	it("mark nothing without this Mac's key", () => {
		const rows = appKeyRows(view([key({ public_key: OWN })]), "devbox", null, NOW);
		expect(rows.map((r) => [r.own, r.sub.startsWith("Sent at setup")])).toEqual([
			[false, true],
		]);
	});
});

describe("warnings", () => {
	it("say since when a device trusts a key this Mac didn't add", () => {
		const v = view([
			key({ public_key: OWN }),
			key({ public_key: OTHER, added_at: NOW - 2 * HOUR }),
			key({ public_key: "third", added_at: NOW - 30 * MIN }),
		]);
		expect(foreignKeyWarning(v, "devbox", state(), NOW)).toBe(
			"devbox has trusted an app key this Mac didn't add since 12:48."
		);
		expect(foreignKeyWarning(v, "ws-alpha", state(), NOW)).toBeNull();
		expect(foreignKeyWarning(v, "devbox", null, NOW)).toBeNull();
		const moving = view([key({ public_key: OWN }), key({ public_key: OLD })]);
		expect(foreignKeyWarning(moving, "devbox", state({ previous: OLD }), NOW)).toBeNull();
	});

	it("say a device no longer trusts this Mac's key, or never did", () => {
		const v = view([]);
		const refused = (trusted_before: boolean, kept_previous = false) =>
			refusalNote(
				v,
				"devbox",
				state({
					refusals: [
						{ device: "devbox", reason: "Unknown key", trusted_before, kept_previous },
					],
				})
			);
		expect(refused(true)).toBe("devbox no longer trusts this Mac's app key: Unknown key");
		expect(refused(false)).toBe("devbox doesn't trust this Mac's app key: Unknown key");
		expect(refused(true, true)).toBe(
			"devbox turned down this Mac's new app key and still uses the one before: Unknown key"
		);
		expect(refusalNote(v, "ws-alpha", state())).toBeNull();
		expect(refusalNote(v, "devbox", null)).toBeNull();
	});

	it("say a device took the first key offered, and whether it was this Mac's", () => {
		const risk =
			"devbox trusted the first app key it was offered, without checking which program offered it.";
		const ours = view([key({ public_key: OWN, origin: "first" })]);
		expect(firstKeyNote(ours, "devbox", state())).toEqual({
			text: `${risk} That was this Mac's key.`,
			warning: false,
		});
		// A key setup sent, or one the device checked, needs no note.
		expect(firstKeyNote(view([key({ public_key: OWN })]), "devbox", state())).toBeNull();

		const theirs = view([
			key({ public_key: OTHER, origin: "first" }),
			key({ public_key: OWN, origin: "approved" }),
		]);
		expect(firstKeyNote(theirs, "devbox", state())).toEqual({
			text: `${risk} It wasn't this Mac's key. Remove it if it isn't yours.`,
			warning: true,
		});
		expect(firstKeyNote(theirs, "devbox", null)).toEqual({ text: risk, warning: true });
	});

	it("say this Mac's unsigned maxd took the first key offered", () => {
		const v = view([key({ public_key: OWN, device: "mac", origin: "first" })]);
		expect(firstKeyNote(v, "mac", state())?.text).toBe(
			"This Mac's maxd isn't signed, so it trusted the first app key it was offered without checking which program offered it. That was this Mac's key."
		);
	});

	it("say how to start over when the first key wasn't this Mac's", () => {
		const startOver =
			"To start over, delete phones/app-keys.json in maxd's data folder on devbox and restart maxd there. It then trusts the first app that connects.";
		const v = view([key({ public_key: OTHER, origin: "first" })]);
		expect(firstKeyNote(v, "devbox", state())?.text).toBe(
			`devbox trusted the first app key it was offered, without checking which program offered it. It wasn't this Mac's key. ${startOver}`
		);

		// A device that never let this Mac in shows it no keys, so all the
		// note can say is how to start over.
		const refused = (trusted_before: boolean, kept_previous = false) =>
			firstKeyNote(
				view([]),
				"devbox",
				state({
					refusals: [
						{ device: "devbox", reason: "Unknown key", trusted_before, kept_previous },
					],
				})
			);
		expect(refused(false)).toEqual({
			text: `If no other Mac of yours uses devbox, another program may have offered it a key before this Mac did. ${startOver}`,
			warning: true,
		});
		expect(refused(true)).toBeNull();
		expect(refused(true, true)).toBeNull();
		expect(firstKeyNote(view([]), "devbox", state())).toBeNull();
	});
});

describe("new key", () => {
	it("lists the devices still on the old key", () => {
		const v = view([]);
		expect(rotationNote(v, state())).toContain("Makes a new key");
		expect(rotationNote(v, state({ previous: OLD, waiting: ["ws-alpha", "devbox"] }))).toBe(
			"Still on the old key: devbox, ws-alpha. A device that's off moves when this Mac next reaches it."
		);
		expect(rotationNote(v, state({ previous: OLD }))).toBe(
			"Every device has moved to the new key."
		);
	});

	it("says why the key changed as the app opened, or that it won't last", () => {
		expect(keyStateNote(state())).toBeNull();
		expect(keyStateNote(state({ remade: "the Keychain item was damaged" }))).toBe(
			"This Mac made a new app key: the Keychain item was damaged"
		);
		expect(keyStateNote(state({ kept: false }))).toContain("lasts until you quit");
	});
});
