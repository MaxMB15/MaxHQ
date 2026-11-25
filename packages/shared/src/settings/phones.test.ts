// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, Phone, PhoneDevice } from "../bindings";
import { device, entities, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	letsPhone,
	needsSwitch,
	pairedPhones,
	pairingDevice,
	reachable,
	withoutPushKey,
} from "./phones";

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

	const key = (device: string, type: "apns" | "fcm" | "github_user"): Entity =>
		({
			entity: "credential",
			id: `cred-${type}-${device}`,
			device,
			kind:
				type === "apns"
					? { type, key_id: "ABC123DEFG", team_id: "TEAM" }
					: type === "fcm"
						? { type, project_id: "maxhq-example", client_email: "push@example.com" }
						: { type, login: "me" },
			stored_at: NOW,
			problem: null,
		}) as Entity;
	const missing = (more: Entity[]) => {
		const v = view(more);
		return withoutPushKey(v, pairedPhones(v)[0]!);
	};

	it("names an iPhone's devices that hold no APNs key", () => {
		expect(
			missing([
				phone({ device: "devbox", service: "apns" }),
				phone({ device: "ws-alpha", service: "apns" }),
				key("devbox", "apns"),
				key("ws-alpha", "fcm"),
			])
		).toEqual({ service: "apns", names: ["ws-alpha"] });
		expect(
			missing([phone({ device: "devbox", service: "apns" }), key("devbox", "apns")])
		).toEqual({ service: "apns", names: [] });
	});

	it("names an Android phone's devices that hold no Firebase key", () => {
		expect(
			missing([
				phone({ device: "devbox", service: "fcm" }),
				phone({ device: "ws-alpha", service: "fcm" }),
				key("devbox", "apns"),
				key("ws-alpha", "fcm"),
			])
		).toEqual({ service: "fcm", names: ["devbox"] });
	});

	it("names only devices with neither key before the phone's token comes", () => {
		expect(
			missing([
				phone({ device: "mac" }),
				phone({ device: "devbox" }),
				phone({ device: "ws-alpha" }),
				key("devbox", "apns"),
				key("ws-alpha", "fcm"),
				key("mac", "github_user"),
			])
		).toEqual({ service: null, names: ["Mac"] });
	});

	it("takes a phone's service from any device that has its token", () => {
		const v = view([
			phone({ device: "devbox" }),
			phone({ device: "ws-alpha", service: "fcm" }),
			phone({ device: "mac", service: null }),
		]);
		expect(pairedPhones(v)[0]?.service).toBe("fcm");
		expect(pairedPhones(view([phone({ device: "devbox" })]))[0]?.service).toBeNull();
	});
});

describe("letsPhone", () => {
	it("is on only for the device that turned the switch on for that phone", () => {
		const v = view([
			phone({ device: "mac", everything: true }),
			phone({ device: "devbox" }),
			phone({ id: "phone-2", device: "ws-alpha", everything: true }),
		]);
		expect(letsPhone(v, "phone-1", "mac")).toBe(true);
		expect(letsPhone(v, "phone-1", "devbox")).toBe(false);
		expect(letsPhone(v, "phone-1", "ws-alpha")).toBe(false);
		expect(letsPhone(v, "phone-2", "ws-alpha")).toBe(true);
	});
});

describe("needsSwitch", () => {
	it("holds back the modes that act without asking", () => {
		expect(needsSwitch("auto")).toBe(true);
		expect(needsSwitch("full_access")).toBe(true);
		expect(needsSwitch("ask")).toBe(false);
		expect(needsSwitch("accept_edits")).toBe(false);
		expect(needsSwitch("plan")).toBe(false);
	});
});
