// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Admission, Entity } from "../bindings";
import { device, entities, link, MIN, NOW } from "../test/build";
import { applySnapshot } from "../view/replica";

import { admissionCard, admissionLink, DONT_ALLOW, openAdmissions } from "./admission";

const TITLE = "Let this program control box-host?";
const RISK = "A program connected to maxd on box-host without MaxHQ's app key.";

/** A card on `device`, as maxd lays it out. */
const card = (over: Partial<Admission> & { id: string }): Admission => ({
	device: "dev-box",
	raised_at: NOW - MIN,
	expires_at: NOW + 9 * MIN,
	question: {
		id: over.id,
		header: null,
		text: `${TITLE}\n\n${RISK}\n\nDevice: box-host`,
		options: [
			{ label: DONT_ALLOW, description: null },
			{ label: "Allow until it disconnects", description: null },
		],
		multi: false,
		other: false,
		secret: false,
		consent: {
			title: TITLE,
			risk: RISK,
			changes: [],
			facts: [
				{ label: "Device", value: "box-host" },
				{ label: "Says it is", value: "maxhq-desktop 0.9.0" },
			],
			rows: [],
			remember: null,
		},
	},
	...over,
});

const viewOf = (cards: Admission[]) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [device({ id: "dev-box", name: "box-host" })],
				links: [link({ id: "box", name: "devbox", device: "dev-box" })],
			}),
			...cards.map((c): Entity => ({ entity: "admission", ...c })),
		],
	});

describe("openAdmissions", () => {
	it("lists the open cards oldest first, on one device or all", () => {
		const view = viewOf([
			card({ id: "b", raised_at: NOW - MIN }),
			card({ id: "a", raised_at: NOW - 3 * MIN }),
			card({ id: "c", device: "dev-other" }),
		]);
		expect(openAdmissions(view).map((c) => c.id)).toEqual(["a", "b", "c"]);
		expect(openAdmissions(view, "dev-box").map((c) => c.id)).toEqual(["a", "b"]);
		expect(openAdmissions(view, "dev-none")).toEqual([]);
	});
});

describe("admissionCard", () => {
	it("lays out the card's consent, Don't allow first", () => {
		const view = viewOf([]);
		const words = admissionCard(view, card({ id: "a" }));
		expect(words).toMatchObject({
			deviceName: "devbox",
			title: TITLE,
			risk: RISK,
			deny: "Don't allow",
			allow: "Allow until it disconnects",
			remember: "Nothing is kept. If it connects again, it asks again.",
		});
		expect(words.facts.map((f) => f.label)).toEqual(["Device", "Says it is"]);
	});

	it("says a yes keeps the key when the card asks to trust one", () => {
		const base = card({ id: "a" });
		const consent = base.question.consent;
		if (!consent) throw new Error("no consent");
		const enroll = card({
			id: "a",
			question: {
				...base.question,
				options: [
					{ label: DONT_ALLOW, description: null },
					{ label: "Trust its key", description: null },
				],
				consent: {
					...consent,
					facts: [...(consent.facts ?? []), { label: "Key", value: "ed25519:abc" }],
				},
			},
		});
		const words = admissionCard(viewOf([]), enroll);
		expect(words.allow).toBe("Trust its key");
		expect(words.remember).toBe(
			"A yes adds the key to App keys in devbox's settings, until you remove it."
		);
	});

	it("reads the plain text when the card lays out no consent", () => {
		const base = card({ id: "a" });
		const words = admissionCard(
			viewOf([]),
			card({ id: "a", question: { ...base.question, consent: null } })
		);
		expect(words.title).toBe(TITLE);
		expect(words.risk).toBe(`${RISK}\n\nDevice: box-host`);
		expect(words.facts).toEqual([]);
	});
});

describe("admissionLink", () => {
	it("opens the page of the link that reaches the card's device", () => {
		const view = viewOf([]);
		expect(admissionLink(view, card({ id: "a" }))).toBe("box");
		expect(admissionLink(view, card({ id: "a", device: "dev-none" }))).toBeNull();
	});
});
