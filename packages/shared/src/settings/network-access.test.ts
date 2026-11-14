// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, NetworkAccess, PhoneSession } from "../bindings";
import { device, entities, MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { awaitingSignIn, networkReady, parsePort, sessionLine, signedIn } from "./network-access";

const DAY = 24 * 60 * MIN;

const session = (over: Partial<PhoneSession> & { id: string }): PhoneSession => ({
	device: "devbox",
	phone: "phone-1",
	created: NOW - 2 * DAY,
	expires: NOW + 28 * DAY,
	last_used: NOW - 4 * MIN,
	...over,
});

const phone = (id: string, name: string, on: string): Entity => ({
	entity: "phone",
	id,
	name,
	ssh_public: "ssh-ed25519 AAAA",
	push_public: "AAAA",
	paired_at: NOW,
	device: on,
	push: [],
});

describe("parsePort", () => {
	it("takes a port from 1 to 65535", () => {
		expect(parsePort(" 7466 ")).toEqual({ ok: true, port: 7466 });
		expect(parsePort("65535")).toEqual({ ok: true, port: 65535 });
	});

	it("says why anything else isn't one", () => {
		expect(parsePort("0")).toEqual({
			ok: false,
			reason: "0 isn't a port. Ports run from 1 to 65535.",
		});
		expect(parsePort("70000")).toMatchObject({ ok: false });
		expect(parsePort("74a")).toMatchObject({ ok: false });
		expect(parsePort("7466 7467")).toEqual({
			ok: false,
			reason: "Type one port, from 1 to 65535.",
		});
		expect(parsePort("")).toMatchObject({ ok: false });
	});
});

const access = (over: Partial<NetworkAccess>): NetworkAccess => ({
	device: "devbox",
	on: true,
	port: 7466,
	hosts: ["192.168.1.30"],
	pin: "mJ3x0q7wTn2cV9bLr4sYk8pZf1hGd6aEu5oWi0XtQeA",
	problem: null,
	...over,
});

describe("networkReady", () => {
	it("needs the switch on, a pin and an address", () => {
		expect(networkReady(access({}))).toBe(true);
		expect(networkReady(access({ on: false }))).toBe(false);
		expect(networkReady(access({ pin: null }))).toBe(false);
		expect(networkReady(access({ hosts: [] }))).toBe(false);
	});
});

describe("signedIn", () => {
	it("lists the phones signed in to a device by name", () => {
		const view = applySnapshot({
			seq: 1,
			entities: [
				...entities({ devices: [device({ id: "devbox" }), device({ id: "mac" })] }),
				phone("phone-1", "Work phone", "devbox"),
				phone("phone-2", "iPhone", "devbox"),
				{ entity: "phone_session", ...session({ id: "s1" }) },
				{ entity: "phone_session", ...session({ id: "s2", phone: "phone-2" }) },
				{ entity: "phone_session", ...session({ id: "s3", phone: "phone-3" }) },
				{ entity: "phone_session", ...session({ id: "s4", device: "mac" }) },
			],
		});
		expect(signedIn(view, "devbox").map((s) => [s.name, s.session.id])).toEqual([
			["A phone", "s3"],
			["iPhone", "s2"],
			["Work phone", "s1"],
		]);
	});
});

describe("awaitingSignIn", () => {
	it("lists the devices ready for a phone that has no sign-in there", () => {
		const view = applySnapshot({
			seq: 1,
			entities: [
				...entities({
					devices: ["devbox", "ws-alpha", "ws-bravo", "mac"].map((id) => device({ id })),
				}),
				...["devbox", "ws-alpha", "ws-bravo", "mac"].map((on) =>
					phone("phone-1", "iPhone", on)
				),
				{ entity: "network_access", ...access({ device: "devbox" }) },
				{ entity: "network_access", ...access({ device: "ws-alpha" }) },
				{ entity: "network_access", ...access({ device: "ws-bravo" }) },
				{ entity: "network_access", ...access({ device: "mac", on: false }) },
				{ entity: "phone_session", ...session({ id: "s1", device: "ws-bravo" }) },
				{ entity: "phone_session", ...session({ id: "s2", phone: "phone-2" }) },
			],
		});
		expect(awaitingSignIn(view, "phone-1").map((d) => d.name)).toEqual(["devbox", "ws-alpha"]);
		expect(awaitingSignIn(view, "phone-2")).toEqual([]);
	});
});

describe("sessionLine", () => {
	it("says when the phone signed in, last used it and when its token ends", () => {
		expect(sessionLine(session({ id: "s1" }), NOW)).toMatch(
			/^Signed in \S+ \d\d:\d\d, last used 4 min ago, expires in 28 d$/
		);
		expect(
			sessionLine(session({ id: "s1", last_used: null, expires: NOW - MIN }), NOW)
		).toMatch(/, never used, expired$/);
	});
});
