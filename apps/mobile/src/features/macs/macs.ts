// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Macs this phone pairs with, as its screens show them. With two or
// more Macs you pick how they show: together, each agent row with a small
// tag naming its Mac, or one at a time, with a switch at the top of the
// agent list. With one Mac nothing changes. The devices added by hand
// form a group of their own.

import { formatList, formatNumber, t } from "@maxhq/shared/i18n";

import type { Agent } from "@maxhq/shared/bindings";
import type { DeviceReach } from "@maxhq/shared/status/reach";
import { openAdmissions } from "@maxhq/shared/ui/admission";
import type { Tables, ViewState } from "@maxhq/shared/view/replica";
import { needsYou } from "@maxhq/shared/view/select";

import type { PairedMac } from "../../data/native-source";

/** How the agent list shows several Macs. */
export type MacsShow = "together" | "one";

/** This phone's choice, and the Mac it shows one at a time. */
export type MacsPref = {
	readonly show: MacsShow;
	/** The keeper of the Mac picked in the switch. */
	readonly pick: string | null;
};

export const DEFAULT_MACS_PREF: MacsPref = { show: "together", pick: null };

/** The choice kept as `text`, with the defaults for what's missing or unreadable. */
export const macsPrefOf = (text: string | null): MacsPref => {
	if (!text) return DEFAULT_MACS_PREF;
	try {
		const kept: unknown = JSON.parse(text);
		if (typeof kept !== "object" || kept === null) return DEFAULT_MACS_PREF;
		const { show, pick } = kept as Record<string, unknown>;
		return {
			show: show === "one" ? "one" : "together",
			pick: typeof pick === "string" ? pick : null,
		};
	} catch {
		return DEFAULT_MACS_PREF;
	}
};

/** The Macs, without the devices added by hand. */
const realMacs = (macs: readonly PairedMac[]): PairedMac[] => macs.filter((mac) => !mac.by_hand);

/** Whether the phone pairs with two or more Macs, so the choice shows. */
export const severalMacs = (macs: readonly PairedMac[]): boolean => realMacs(macs).length >= 2;

/** The name a group shows: its Mac's, or "Added by hand". */
export const groupName = (mac: PairedMac): string => (mac.by_hand ? t("Added by hand") : mac.name);

// Past this many letters a tag takes the name's first word.
const SHORT = 12;

/** "Home Mac" as "Home", a long name as its first word or cut. */
const shorten = (name: string): string => {
	const bare = name.replace(/\s+mac$/i, "").trim() || name;
	if (bare.length <= SHORT) return bare;
	const first = bare.split(/\s+/)[0] ?? bare;
	return first.length >= 3 && first.length <= SHORT ? first : `${bare.slice(0, SHORT - 1)}…`;
};

/** Numbers the names that repeat, such as "Mac" and "Mac 2". */
const unique = (names: readonly string[]): string[] => {
	const seen = new Map<string, number>();
	return names.map((name) => {
		const times = (seen.get(name) ?? 0) + 1;
		seen.set(name, times);
		return times === 1 ? name : `${name} ${formatNumber(times)}`;
	});
};

/**
 * Each Mac's short name for tags and the switch, by keeper. When two
 * short names would match, every Mac keeps its full name.
 */
export const shortNames = (macs: readonly PairedMac[]): ReadonlyMap<string, string> => {
	const real = realMacs(macs);
	const short = real.map((mac) => shorten(mac.name));
	const names = unique(new Set(short).size < short.length ? real.map((mac) => mac.name) : short);
	return new Map(real.map((mac, i) => [mac.keeper, names[i]!]));
};

/** Which Mac each device and link belongs to: the first Mac whose book lists it. */
export type MacPlaces = {
	readonly byDevice: ReadonlyMap<string, PairedMac>;
	readonly byLink: ReadonlyMap<string, PairedMac>;
};

export const macPlaces = (macs: readonly PairedMac[]): MacPlaces => {
	const byDevice = new Map<string, PairedMac>();
	const byLink = new Map<string, PairedMac>();
	for (const mac of macs) {
		for (const device of mac.devices) {
			if (!byDevice.has(device.device)) byDevice.set(device.device, mac);
			if (!byLink.has(device.link)) byLink.set(device.link, mac);
		}
	}
	return { byDevice, byLink };
};

/** The Mac a device belongs to, through its book or the link that reaches it. */
export const macOfDevice = (
	view: ViewState,
	places: MacPlaces,
	device: string
): PairedMac | undefined => {
	const mac = places.byDevice.get(device);
	if (mac) return mac;
	for (const link of view.tables.device_link.values()) {
		if (link.device === device) {
			const owner = places.byLink.get(link.id);
			if (owner) return owner;
		}
	}
	return undefined;
};

/** The Mac an agent's device belongs to. */
export const macOfAgent = (
	view: ViewState,
	places: MacPlaces,
	agent: Agent
): PairedMac | undefined => {
	const workspace = view.tables.workspace.get(agent.workspace);
	return workspace ? macOfDevice(view, places, workspace.device) : undefined;
};

/** How the agent list shows the Macs right now. */
export type MacsMode =
	/** One Mac or none: no tags, no switch. */
	| { readonly kind: "single" }
	/** Several together, each row tagged with its Mac's short name. */
	| { readonly kind: "together"; readonly short: ReadonlyMap<string, string> }
	/** One Mac at a time, the one picked in the switch. */
	| { readonly kind: "one"; readonly picked: PairedMac };

/** The groups the switch offers: each Mac with devices, then those added by hand. */
export const switchGroups = (macs: readonly PairedMac[]): PairedMac[] => [
	...macs.filter((mac) => !mac.by_hand && mac.devices.length > 0),
	...macs.filter((mac) => mac.by_hand && mac.devices.length > 0),
];

export const macsMode = (macs: readonly PairedMac[], pref: MacsPref): MacsMode => {
	if (!severalMacs(macs)) return { kind: "single" };
	if (pref.show === "together") return { kind: "together", short: shortNames(macs) };
	const groups = switchGroups(macs);
	const picked = groups.find((mac) => mac.keeper === pref.pick) ?? groups[0];
	return picked ? { kind: "one", picked } : { kind: "together", short: shortNames(macs) };
};

/** The tag an agent's row shows: its Mac's short name, while several Macs show together. */
export const tagOf = (
	view: ViewState,
	places: MacPlaces,
	mode: MacsMode,
	agent: Agent
): string | null => {
	if (mode.kind !== "together") return null;
	const mac = macOfAgent(view, places, agent);
	return mac ? (mode.short.get(mac.keeper) ?? null) : null;
};

/** One segment of the switch: a Mac, its label, and how much waits on you there. */
export type MacSegment = {
	readonly mac: PairedMac;
	readonly label: string;
	/** Agents waiting on you, and programs asking to get in. */
	readonly waiting: number;
};

/** The switch's segments, counted on the whole view so each shows what waits on you. */
export const macSegments = (
	view: ViewState,
	places: MacPlaces,
	macs: readonly PairedMac[]
): MacSegment[] => {
	const counts = new Map<string, number>();
	const count = (mac: PairedMac | undefined) => {
		if (mac) counts.set(mac.keeper, (counts.get(mac.keeper) ?? 0) + 1);
	};
	for (const { agent } of needsYou(view)) count(macOfAgent(view, places, agent));
	for (const card of openAdmissions(view)) count(macOfDevice(view, places, card.device));
	const short = shortNames(macs);
	return switchGroups(macs).map((mac) => ({
		mac,
		label: mac.by_hand ? t("By hand") : (short.get(mac.keeper) ?? mac.name),
		waiting: counts.get(mac.keeper) ?? 0,
	}));
};

const filtered = <V>(table: ReadonlyMap<string, V>, keep: (value: V) => boolean) =>
	new Map([...table].filter(([, value]) => keep(value)));

/**
 * The view with only `mac`'s devices, their agents, programs asking to get
 * in and notes. What no Mac's book lists shows under every Mac, so nothing
 * hides.
 */
export const macView = (view: ViewState, places: MacPlaces, mac: PairedMac): ViewState => {
	const owners = new Map<string, boolean>();
	const ours = (device: string): boolean => {
		let mine = owners.get(device);
		if (mine === undefined) {
			const owner = macOfDevice(view, places, device);
			mine = owner === undefined || owner.keeper === mac.keeper;
			owners.set(device, mine);
		}
		return mine;
	};
	const { tables } = view;
	const workspaces = filtered(tables.workspace, (w) => ours(w.device));
	const shown: Tables = {
		...tables,
		device_link: filtered(tables.device_link, (link) => {
			const owner =
				places.byLink.get(link.id) ??
				(link.device !== null ? macOfDevice(view, places, link.device) : undefined);
			return owner === undefined || owner.keeper === mac.keeper;
		}),
		workspace: workspaces,
		agent: filtered(
			tables.agent,
			(agent) => workspaces.has(agent.workspace) || !tables.workspace.has(agent.workspace)
		),
		admission: filtered(tables.admission, (card) => ours(card.device)),
		project_notes: filtered(tables.project_notes, (notes) => ours(notes.device)),
	};
	return { seq: view.seq, tables: shown };
};

/** The devices under one heading of the Devices screen. */
export type DeviceGroup = {
	readonly key: string;
	readonly name: string;
	readonly devices: readonly DeviceReach[];
};

/**
 * The Devices screen's groups: each Mac's devices under its name, those
 * added by hand, then any no book lists. In one-at-a-time, only the picked
 * Mac's.
 */
export const deviceGroups = (
	devices: readonly DeviceReach[],
	macs: readonly PairedMac[],
	mode: MacsMode
): DeviceGroup[] => {
	const places = macPlaces(macs);
	const groups = new Map<string, DeviceReach[]>();
	const others: DeviceReach[] = [];
	for (const device of devices) {
		const mac = places.byLink.get(device.id);
		if (!mac) {
			others.push(device);
			continue;
		}
		const group = groups.get(mac.keeper) ?? [];
		group.push(device);
		groups.set(mac.keeper, group);
	}
	const named = [...realMacs(macs), ...macs.filter((mac) => mac.by_hand)].flatMap((mac) => {
		const shown = groups.get(mac.keeper);
		if (!shown) return [];
		if (mode.kind === "one" && mode.picked.keeper !== mac.keeper) return [];
		return [{ key: mac.keeper, name: groupName(mac), devices: shown }];
	});
	if (others.length === 0) return named;
	return [
		...named,
		{
			key: "others",
			name: named.length > 0 ? t("Other devices") : t("Devices"),
			devices: others,
		},
	];
};

// How many device names a Paired Macs row shows before it counts the rest.
const NAMED = 3;

/** A Paired Macs row's note, such as "3 devices · Mac, devbox, build-box". */
export const macRowNote = (mac: PairedMac): string => {
	const count = t("{{count}} devices", { count: mac.devices.length });
	if (mac.devices.length === 0) return count;
	const names = mac.devices.map((device) => device.name);
	const list = (shown: readonly string[]) => formatList(shown, { type: "unit", style: "short" });
	const listed =
		names.length <= NAMED + 1
			? list(names)
			: t("{{names}} and {{count}} more", {
					names: list(names.slice(0, NAMED)),
					count: names.length - NAMED,
				});
	return `${count} · ${listed}`;
};

/**
 * What Forget asks before it forgets `mac`: what goes, what stays, and
 * where on the Mac to remove this phone too, since the Mac still lets its
 * key in until then.
 */
export const forgetWords = (
	mac: PairedMac,
	macs: readonly PairedMac[]
): { readonly title: string; readonly detail: string } => {
	const going = mac.devices.filter((device) => !device.elsewhere).map((device) => device.name);
	const staying = mac.devices.filter((device) => device.elsewhere).map((device) => device.name);
	const name = groupName(mac);
	const lines = [
		going.length > 0
			? t("This phone forgets {{devices}} and signs out of them.", {
					devices: formatList(going),
					count: going.length,
				})
			: t("Every device stays, since another Mac lists each one."),
	];
	if (staying.length > 0) {
		lines.push(
			t("{{devices}} stay, since another Mac lists them.", {
				devices: formatList(staying),
				count: staying.length,
			})
		);
	}
	if (macs.some((other) => other.keeper !== mac.keeper && !other.by_hand)) {
		lines.push(t("Your other Macs stay paired."));
	}
	if (!mac.by_hand) {
		lines.push(
			t(
				"To end this phone's access there too, remove it on {{mac}} in Settings, under Phones.",
				{ mac: name }
			)
		);
	}
	return {
		title: mac.by_hand
			? t("Forget the devices added by hand?")
			: t("Forget {{mac}}?", { mac: name }),
		detail: lines.join(" "),
	};
};

/**
 * What the scan screen says about a code: that it adds a Mac, or that it
 * comes from `from`, a paired Mac, and replaces its devices. Nothing before
 * the first Mac.
 */
export const pairNote = (from: string | null, macs: readonly PairedMac[]): string | null => {
	if (from !== null) {
		return t("This code is from {{mac}}. Pairing replaces this Mac's devices.", { mac: from });
	}
	return realMacs(macs).length > 0
		? t("This code adds a Mac. Your other Macs stay paired.")
		: null;
};

/**
 * What Settings says under this phone's name: where it comes from. No device
 * can rename a phone, and pairing again sets the name anew.
 */
export const nameNote = (): string =>
	t(
		"How your devices list this phone, from This phone's name when you paired it. Pair again to change it."
	);

/** This phone's name as its devices list it, whichever device tells it. */
export const phoneName = (view: ViewState, phone: string): string | undefined => {
	for (const entry of view.tables.phone.values()) {
		if (entry.id === phone) return entry.name;
	}
	return undefined;
};
