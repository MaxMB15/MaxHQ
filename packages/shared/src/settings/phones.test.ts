// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, Phone, PhoneDevice } from "../bindings";
import { device, entities, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { pairedPhones, pairingDevice, reachable } from "./phones";

const phone = (over: Partial<Phone> & { device: string }): Entity => ({
	entity: "phone",
	id: "phone-1",
	name: "iPhone",
	ssh_public: "ssh-ed25519 AAAA",
	push_public: "AAAA",
	paired_at: NOW,
	push: [],
	...over,
});

const view = (more: Entity[] = []) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [
					device({ id: "mac", name: "Mac", os: "macos" }),
					device({ id: "devbox", name: "devbox" }),
					device({ id: "ws-alpha", name: "ws-alpha" }),
				],
			}),
			...more,
		],
	});

const found = (device: string, note: string | null = null): PhoneDevice => ({
	device,
	name: device,
	address: null,
	note,
});

describe("phones", () => {
	it("lists each phone once with every device that has it", () => {
		const phones = pairedPhones(
			view([
				phone({ device: "ws-alpha" }),
				phone({ device: "devbox", name: "Work phone", paired_at: NOW + 1 }),
				phone({ device: "devbox", id: "phone-2", name: "Another" }),
			])
		);
		expect(phones.map((p) => [p.name, p.devices.map((d) => d.name)])).toEqual([
			["Another", ["devbox"]],
			["Work phone", ["devbox", "ws-alpha"]],
		]);
	});

	it("says a phone may do everything when any device lets it", () => {
		const off = pairedPhones(
			view([phone({ device: "devbox" }), phone({ device: "ws-alpha" })])
		);
		expect(off[0]?.everything).toBe(false);
		const [on] = pairedPhones(
			view([phone({ device: "devbox", everything: true }), phone({ device: "ws-alpha" })])
		);
		expect(on?.everything).toBe(true);
		expect(on?.devices.map((d) => [d.name, d.everything])).toEqual([
			["devbox", true],
			["ws-alpha", false],
		]);
	});

	it("ticks only the devices a phone can reach", () => {
		expect(reachable([found("devbox"), found("mac", "No address")])).toEqual(["devbox"]);
	});

	it("pairs through a device that stays on before a Mac", () => {
		const v = view();
		expect(pairingDevice(v, ["mac", "devbox"])).toBe("devbox");
		expect(pairingDevice(v, ["mac"])).toBe("mac");
		expect(pairingDevice(v, [])).toBeNull();
	});
});
