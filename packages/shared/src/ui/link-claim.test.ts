// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { LinkClaim, LinkClaimDetails } from "../bindings";
import { deviceConnection } from "../status/connection";
import { device, entities, link, MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import { linkContext, linkForDevice } from "../view/select";

import {
	claimCard,
	claimNames,
	heldConnection,
	heldLabel,
	isHeld,
	WHERE_TO_CHANGE,
} from "./link-claim";

const asking: LinkClaim = { device: "dev-box", known_by: "box", state: "asking", since: NOW - MIN };

/** devbox reaches dev-box; spare's hello says it's dev-box too. */
const viewWith = (claim: LinkClaim | null) =>
	applySnapshot({
		seq: 1,
		entities: entities({
			devices: [device({ id: "dev-box", name: "box-host" })],
			links: [
				link({ id: "box", name: "devbox", device: "dev-box" }),
				link({ id: "spare", name: "spare", device: null, claim }),
			],
		}),
	});

const spareOf = (view: ReturnType<typeof viewWith>) => {
	const found = view.tables.device_link.get("spare");
	if (!found) throw new Error("no spare link");
	return found;
};

const details: LinkClaimDetails = {
	link: {
		link: "spare",
		name: "spare",
		addresses: ["max@203.0.113.7"],
		fingerprints: ["ED25519 SHA256:bad"],
		first_seen: NOW - MIN,
	},
	known_by: {
		link: "box",
		name: "devbox",
		addresses: ["max@devbox", "max@devbox.example.ts.net"],
		fingerprints: ["ED25519 SHA256:good"],
		first_seen: null,
	},
	device_keys: ["ED25519 SHA256:good"],
	where_to_change: WHERE_TO_CHANGE,
};

describe("claimNames", () => {
	it("names the link, the device by the link that reaches it, and that link", () => {
		const view = viewWith(asking);
		expect(claimNames(view, spareOf(view))).toEqual({
			link: "spare",
			device: "devbox",
			knownBy: "devbox",
			different: false,
		});
	});

	it("is null for a link that isn't held", () => {
		const view = viewWith(null);
		expect(claimNames(view, spareOf(view))).toBeNull();
	});

	it("says when you said it's a different device", () => {
		const view = viewWith({ ...asking, state: "different" });
		expect(claimNames(view, spareOf(view))?.different).toBe(true);
	});
});

describe("claimCard", () => {
	const names = { link: "spare", device: "devbox", knownBy: "devbox", different: false };

	it("asks plainly, names the risk, and says where the answer is kept", () => {
		const card = claimCard(names, null, NOW);
		expect(card.title).toBe("spare says it is devbox, which devbox already knows");
		expect(card.risk).toContain("sign-in tokens and actions meant for devbox");
		expect(card.risk).toContain("Until you answer, nothing goes through it.");
		expect(card.remember).toBe(
			`MaxHQ keeps your answer. You can change it under ${WHERE_TO_CHANGE}.`
		);
		expect([card.same, card.different]).toEqual(["Same device", "Different device"]);
		expect(card.sides).toEqual([]);
	});

	it("shows both links' addresses, host keys and first-seen times", () => {
		const card = claimCard(names, details, NOW);
		expect(card.sides.map((s) => s.title)).toEqual(["spare", "devbox", "Kept for devbox"]);
		expect(card.sides[0]?.facts).toEqual([
			{ label: "Address", value: "max@203.0.113.7" },
			{ label: "Host key", value: "ED25519 SHA256:bad" },
			{ label: "First seen", value: "14:47" },
		]);
		expect(card.sides[1]?.facts).toEqual([
			{ label: "Addresses", value: "max@devbox, max@devbox.example.ts.net" },
			{ label: "Host key", value: "ED25519 SHA256:good" },
			{ label: "First seen", value: "Not known" },
		]);
	});

	it("names the kept host keys when no other link knows the device", () => {
		const card = claimCard({ ...names, knownBy: null }, { ...details, known_by: null }, NOW);
		expect(card.title).toBe("spare says it is devbox, whose host keys MaxHQ already keeps");
		expect(card.sides.map((s) => s.title)).toEqual(["spare", "Kept for devbox"]);
	});
});

describe("held links", () => {
	it("label the row and the dot while they wait or stay cut off", () => {
		const view = viewWith(asking);
		const spare = spareOf(view);
		expect(isHeld(spare)).toBe(true);
		expect(heldLabel(spare)).toBe("Needs your answer");
		const waiting = deviceConnection(linkContext(view, spare), NOW);
		expect(waiting.look).toBe("connecting");
		expect(waiting.card.title).toBe("spare waits for your answer");
		expect(waiting.card.body).toContain("It says it is devbox.");

		const cut = viewWith({ ...asking, state: "different" });
		expect(heldLabel(spareOf(cut))).toBe("Cut off");
		const off = deviceConnection(linkContext(cut, spareOf(cut)), NOW);
		expect(off.look).toBe("unreachable");
		expect(off.card.title).toBe("spare is cut off");
	});

	it("leave other links alone", () => {
		const view = viewWith(null);
		expect(heldLabel(spareOf(view))).toBeNull();
		expect(heldConnection(spareOf(view), undefined)).toBeUndefined();
	});
});

describe("linkForDevice", () => {
	it("never picks a held link, even a connected one", () => {
		const view = applySnapshot({
			seq: 1,
			entities: entities({
				devices: [device({ id: "dev-box" })],
				links: [
					link({
						id: "box",
						device: "dev-box",
						state: { state: "unreachable", reason: "down", last_seen: null },
					}),
					link({ id: "spare", device: "dev-box", claim: asking }),
				],
			}),
		});
		expect(linkForDevice(view, "dev-box")?.id).toBe("box");
	});

	it("prefers a connected route to the same device", () => {
		const view = applySnapshot({
			seq: 1,
			entities: entities({
				devices: [device({ id: "dev-box" })],
				links: [
					link({
						id: "box",
						device: "dev-box",
						state: { state: "unreachable", reason: "down", last_seen: null },
					}),
					link({ id: "second", device: "dev-box" }),
				],
			}),
		});
		expect(linkForDevice(view, "dev-box")?.id).toBe("second");
	});

	it("finds nothing when the only link is held", () => {
		const view = applySnapshot({
			seq: 1,
			entities: entities({
				devices: [device({ id: "dev-box" })],
				links: [link({ id: "spare", device: "dev-box", claim: asking })],
			}),
		});
		expect(linkForDevice(view, "dev-box")).toBeUndefined();
	});
});
