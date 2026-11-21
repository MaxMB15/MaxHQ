// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Admission, Entity } from "@maxhq/shared/bindings";
import { deviceReach } from "@maxhq/shared/status/reach";
import { agent, entities, link, MIN, NOW, workspace } from "@maxhq/shared/test/build";
import { all, applySnapshot } from "@maxhq/shared/view/replica";
import { needsYou } from "@maxhq/shared/view/select";

import type { PairedDevice, PairedMac } from "../../data/native-source";
import {
	DEFAULT_MACS_PREF,
	deviceGroups,
	forgetWords,
	macPlaces,
	macRowNote,
	macSegments,
	macsMode,
	macsPrefOf,
	macView,
	nameNote,
	pairNote,
	phoneName,
	severalMacs,
	shortNames,
	tagOf,
} from "./macs";

const paired = (name: string, elsewhere = false): PairedDevice => ({
	device: `dev-${name}`,
	link: `link-${name}`,
	name,
	elsewhere,
});

const HOME: PairedMac = {
	keeper: "home-mac",
	name: "Home Mac",
	by_hand: false,
	devices: [paired("mac"), paired("devbox"), paired("shared", true)],
};
const WORK: PairedMac = {
	keeper: "work-mac",
	name: "Work Mac",
	by_hand: false,
	devices: [paired("build")],
};
const BY_HAND: PairedMac = {
	keeper: "by-hand",
	name: "the Mac",
	by_hand: true,
	devices: [paired("pi")],
};
const MACS = [HOME, WORK, BY_HAND];

/** An admission card on `device`, with only what the list reads. */
const card = (id: string, device: string): Entity =>
	({
		entity: "admission",
		id,
		device,
		raised_at: NOW - MIN,
		expires_at: NOW + MIN,
	}) as unknown as Admission & Entity;

/** Each paired device's link and workspace, an agent waiting on Home and Work, one idle on Work. */
const view = applySnapshot({
	seq: 1,
	entities: [
		...entities({
			links: ["mac", "devbox", "shared", "build", "pi"].map((name) =>
				link({ id: `link-${name}`, name, device: `dev-${name}` })
			),
			workspaces: ["mac", "build", "pi"].map((name) =>
				workspace({ id: `w-${name}`, device: `dev-${name}` })
			),
			agents: [
				agent({
					id: "home-ask",
					workspace: "w-mac",
					activity: { state: "needs_input", attention: "att-1" },
				}),
				agent({
					id: "work-ask",
					workspace: "w-build",
					activity: { state: "needs_input", attention: "att-2" },
				}),
				agent({ id: "work-idle", workspace: "w-build" }),
				agent({ id: "pi-idle", workspace: "w-pi" }),
			],
		}),
		card("card-1", "dev-build"),
	],
});

const places = macPlaces(MACS);
const together = macsMode(MACS, DEFAULT_MACS_PREF);
const ids = (agents: readonly { id: string }[]) => agents.map((a) => a.id).sort();

describe("the Settings choice", () => {
	it("reads what it kept, with Together for anything else", () => {
		expect(macsPrefOf(null)).toEqual(DEFAULT_MACS_PREF);
		expect(macsPrefOf("not json")).toEqual(DEFAULT_MACS_PREF);
		expect(macsPrefOf('{"show":"one","pick":"work-mac"}')).toEqual({
			show: "one",
			pick: "work-mac",
		});
		expect(macsPrefOf('{"show":"sideways","pick":3}')).toEqual(DEFAULT_MACS_PREF);
	});

	it("shows only with two or more Macs, not counting the devices added by hand", () => {
		expect(severalMacs(MACS)).toBe(true);
		expect(severalMacs([HOME, BY_HAND])).toBe(false);
		expect(macsMode([HOME, BY_HAND], { show: "one", pick: null })).toEqual({ kind: "single" });
	});

	it("shows one Mac at a time, the first when the pick is gone", () => {
		const one = macsMode(MACS, { show: "one", pick: "work-mac" });
		expect(one).toEqual({ kind: "one", picked: WORK });
		expect(macsMode(MACS, { show: "one", pick: "gone-mac" })).toEqual({
			kind: "one",
			picked: HOME,
		});
		expect(together.kind).toBe("together");
	});
});

describe("tags", () => {
	it("name each Mac short, and in full when the short names match", () => {
		expect([...shortNames(MACS)]).toEqual([
			["home-mac", "Home"],
			["work-mac", "Work"],
		]);
		const twins = [HOME, { ...WORK, keeper: "home-2", name: "Home mac" }];
		expect([...shortNames(twins).values()]).toEqual(["Home Mac", "Home mac"]);
		const same = [HOME, { ...WORK, keeper: "home-2", name: "Home Mac" }];
		expect([...shortNames(same).values()]).toEqual(["Home Mac", "Home Mac 2"]);
		const long = [HOME, { ...WORK, name: "Studio MacBook Pro" }];
		expect(shortNames(long).get("work-mac")).toBe("Studio");
	});

	it("show on agent rows only while several Macs show together", () => {
		const of = (id: string, mode = together) =>
			tagOf(view, places, mode, view.tables.agent.get(id)!);
		expect(of("home-ask")).toBe("Home");
		expect(of("work-idle")).toBe("Work");
		// The devices added by hand have no Mac to name.
		expect(of("pi-idle")).toBeNull();
		expect(of("home-ask", macsMode(MACS, { show: "one", pick: null }))).toBeNull();
		expect(of("home-ask", macsMode([HOME], DEFAULT_MACS_PREF))).toBeNull();
	});
});

describe("one at a time", () => {
	it("counts what waits on you on each Mac, programs asking to get in included", () => {
		expect(
			macSegments(view, places, MACS).map(({ mac, label, waiting }) => [
				mac.keeper,
				label,
				waiting,
			])
		).toEqual([
			["home-mac", "Home", 1],
			["work-mac", "Work", 2],
			["by-hand", "By hand", 0],
		]);
	});

	it("shows only the picked Mac's agents, devices and cards", () => {
		const work = macView(view, places, WORK);
		expect(ids(all(work, "agent"))).toEqual(["work-ask", "work-idle"]);
		expect(ids(all(work, "device_link"))).toEqual(["link-build"]);
		expect(ids(all(work, "admission"))).toEqual(["card-1"]);
		expect(needsYou(work).map(({ agent }) => agent.id)).toEqual(["work-ask"]);

		const home = macView(view, places, HOME);
		expect(ids(all(home, "agent"))).toEqual(["home-ask"]);
		expect(ids(all(home, "device_link"))).toEqual(["link-devbox", "link-mac", "link-shared"]);
		expect(all(home, "admission")).toEqual([]);
	});

	it("keeps what no Mac lists under every Mac", () => {
		const stray = applySnapshot({
			seq: 2,
			entities: entities({
				links: [link({ id: "link-stray", device: "dev-stray" })],
				workspaces: [workspace({ id: "w-stray", device: "dev-stray" })],
				agents: [agent({ id: "stray", workspace: "w-stray" })],
			}),
		});
		expect(ids(all(macView(stray, places, WORK), "agent"))).toEqual(["stray"]);
		expect(ids(all(macView(stray, places, HOME), "device_link"))).toEqual(["link-stray"]);
	});
});

describe("the Devices screen", () => {
	it("groups devices under each Mac's name, those added by hand, then the rest", () => {
		const stray = applySnapshot({
			seq: 2,
			entities: entities({
				links: [
					...["build", "pi", "mac"].map((name) =>
						link({ id: `link-${name}`, name, device: `dev-${name}` })
					),
					link({ id: "link-stray", name: "stray", device: "dev-stray" }),
				],
			}),
		});
		const devices = deviceReach(stray, NOW);
		const groups = deviceGroups(devices, MACS, together);
		expect(groups.map((g) => [g.name, g.devices.map((d) => d.name)])).toEqual([
			["Home Mac", ["mac"]],
			["Work Mac", ["build"]],
			["Added by hand", ["pi"]],
			["Other devices", ["stray"]],
		]);
		const one = deviceGroups(devices, MACS, macsMode(MACS, { show: "one", pick: "work-mac" }));
		expect(one.map((g) => g.name)).toEqual(["Work Mac", "Other devices"]);
		// Without the phone's core, there are no Macs to group by.
		expect(deviceGroups(devices, [], { kind: "single" }).map((g) => g.name)).toEqual([
			"Devices",
		]);
	});
});

describe("Paired Macs", () => {
	it("notes each Mac's devices", () => {
		expect(macRowNote(HOME)).toBe("3 devices · mac, devbox, shared");
		expect(macRowNote(WORK)).toBe("1 device · build");
		expect(macRowNote({ ...WORK, devices: [] })).toBe("0 devices");
		const many = ["mac", "devbox", "build", "pi", "box"].map((name) => paired(name));
		expect(macRowNote({ ...HOME, devices: many })).toBe(
			"5 devices · mac, devbox, build and 2 more"
		);
	});

	it("says what Forget takes, what stays, and that the other Macs stay paired", () => {
		expect(forgetWords(HOME, MACS)).toEqual({
			title: "Forget Home Mac?",
			detail:
				"This phone forgets mac and devbox and signs out of them. " +
				"shared stays, since another Mac lists it. Your other Macs stay paired. " +
				"To end this phone's access there too, remove it on Home Mac in Settings, under Phones.",
		});
		expect(forgetWords(WORK, [WORK]).detail).toBe(
			"This phone forgets build and signs out of it. " +
				"To end this phone's access there too, remove it on Work Mac in Settings, under Phones."
		);
	});

	it("leaves out removing the phone on a Mac for the devices added by hand", () => {
		const words = forgetWords(BY_HAND, [BY_HAND]);
		expect(words.title).toBe("Forget the devices added by hand?");
		expect(words.detail).not.toContain("Settings");
	});

	it("tells a code that adds a Mac from one that replaces a paired Mac's devices", () => {
		expect(pairNote(null, [])).toBeNull();
		expect(pairNote(null, [HOME])).toBe("This code adds a Mac. Your other Macs stay paired.");
		expect(pairNote("Home Mac", [HOME])).toBe(
			"This code is from Home Mac. Pairing replaces this Mac's devices."
		);
	});

	it("says the phone's name comes from pairing, as no device can rename it", () => {
		expect(nameNote()).toBe(
			"How your devices list this phone, from This phone's name when you paired it. Pair again to change it."
		);
	});

	it("finds the phone's name whichever device lists it", () => {
		// The view keys a phone by phone and device, so its ID alone never matches.
		const listed = applySnapshot({
			seq: 1,
			entities: [
				{
					entity: "phone",
					id: "phone-1",
					device: "dev-build",
					name: "iPhone",
				} as unknown as Entity,
			],
		});
		expect(phoneName(listed, "phone-1")).toBe("iPhone");
		expect(phoneName(listed, "phone-2")).toBeUndefined();
	});
});
