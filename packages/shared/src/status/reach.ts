// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How this client reaches its devices, for a list of them and for the
// banner over a screen while it reaches none. The banner shows
// offline, reconnecting, or showing what it saved while nothing answers.

import { t } from "../i18n";

import {
	type AgentId,
	type DeviceLink,
	PROTOCOL_VERSION,
	type Route,
	type RouteChoice,
	type Timestamp,
} from "../bindings";
import { all, type ViewState } from "../view/replica";
import { linkContext, linkForDevice } from "../view/select";

import { type ConnectionLook, deviceConnection, networkAddress } from "./connection";
import { formatAgo, plural } from "./format";

/** What a phone's link says once a device ends its sign-in, as the client says it. */
export const SIGN_IN_ENDED = "the device ended this phone's sign-in, so pair the phone again";

/** What a phone's link says, after the device's name, once the device turns down the phone's key. */
export const NO_LONGER_KNOWN = "no longer knows this phone. Pair it again from the Mac.";

/** Whether a link's reason says only pairing the phone again reaches its device. */
const saysPairAgain = (reason: string): boolean =>
	reason === SIGN_IN_ENDED || reason.endsWith(` ${NO_LONGER_KNOWN}`);

/**
 * A route in words, such as "dev@devbox:22", "Network, 192.168.1.20" or
 * "On this device".
 */
export const routeLabel = (route: Route): string => {
	if (route.type === "local") return t("On this device");
	const address = networkAddress(route.host);
	return address === null
		? route.host.replace(/^ssh:\/\//, "")
		: t("Network, {{address}}", { address: address });
};

/**
 * A link's routes in the order they're tried. A link that lists none, or
 * an empty list, has just the one it's on.
 */
export const linkRoutes = (link: DeviceLink): readonly RouteChoice[] =>
	link.routes?.length ? link.routes : [{ route: link.route, last_good_here: false }];

/**
 * A note when the device's `maxd` speaks a newer or older protocol than this
 * app, or null when it's the same or the device hasn't said yet.
 */
export const versionNote = (link: DeviceLink): string | null => {
	const theirs = link.protocol ?? null;
	if (theirs === null || theirs === PROTOCOL_VERSION) return null;
	return theirs > PROTOCOL_VERSION
		? t("maxd on {{link_name}} is newer than this app; update the app for its new features", {
				link_name: link.name,
			})
		: t("maxd on {{link_name}} is older than this app; update it for the app's new features", {
				link_name: link.name,
			});
};

/** One of a device's routes. */
export type ReachRoute = {
	readonly label: string;
	/** The route the client is on now, or tries first. */
	readonly current: boolean;
	/** Whether it last worked on the network this client is on now. */
	readonly lastGoodHere: boolean;
};

/** A device, and whether and how this client reaches it. */
export type DeviceReach = {
	readonly id: string;
	readonly name: string;
	readonly look: ConnectionLook;
	/** Such as "Connected to devbox", "Can't reach devbox" or "Pair again". */
	readonly title: string;
	/** Why, or since when. */
	readonly detail: string;
	/** Whether `maxd` answers on it now. */
	readonly reached: boolean;
	/** The route it's reached by now, when it is. */
	readonly via: string | null;
	/** Every route, in the order they're tried. */
	readonly routes: readonly ReachRoute[];
	/** Whether the device ended the phone's sign-in or no longer knows the phone, so only pairing again reaches it. */
	readonly pairAgain: boolean;
	/** From `versionNote`. */
	readonly version: string | null;
};

const routesOf = (link: DeviceLink): ReachRoute[] => {
	const current = routeLabel(link.route);
	return linkRoutes(link).map((choice) => {
		const label = routeLabel(choice.route);
		return { label, current: label === current, lastGoodHere: choice.last_good_here };
	});
};

/** Every device this client has a link to, by name. */
export const deviceReach = (view: ViewState, now: Timestamp): DeviceReach[] =>
	all(view, "device_link")
		.map((link) => {
			const { look, card } = deviceConnection(linkContext(view, link), now);
			const reached = link.state.state === "connected";
			const pairAgain =
				link.state.state === "unreachable" && saysPairAgain(link.state.reason);
			return {
				id: link.id,
				name: link.name,
				look,
				title: pairAgain ? t("Pair again") : card.title,
				detail: card.body ?? "",
				reached,
				via: reached ? routeLabel(link.route) : null,
				routes: routesOf(link),
				pairAgain,
				version: versionNote(link),
			};
		})
		.sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));

/**
 * How long after a link connects the device's state may still be on its
 * way. The client says it's connected a round trip before the snapshot
 * lands.
 */
export const SYNC_GRACE_MS = 3_000;

/** Whether the link's device has answered with what it holds, or had time to. */
export const linkSynced = (link: DeviceLink, now: Timestamp): boolean =>
	link.state.state === "connected" && now - link.state.since >= SYNC_GRACE_MS;

/**
 * Whether what the devices hold may still be on its way: a link hasn't
 * tried yet, is dialing, or has only just connected. A link that can't
 * reach its device isn't syncing.
 */
export const stillSyncing = (view: ViewState, now: Timestamp): boolean =>
	all(view, "device_link").some(
		(link) => link.state.state !== "unreachable" && !linkSynced(link, now)
	);

/** Whether the device the agent runs on has synced, so what the view says of it is current. */
export const agentSynced = (view: ViewState, agent: AgentId, now: Timestamp): boolean => {
	const found = view.tables.agent.get(agent);
	const workspace = found ? view.tables.workspace.get(found.workspace) : undefined;
	const link = workspace ? linkForDevice(view, workspace.device) : undefined;
	return link !== undefined && linkSynced(link, now);
};

/** The banner over a screen while the client reaches none of its devices. */
export type ConnectionBanner = {
	readonly tone: "offline" | "reconnecting" | "cached";
	readonly title: string;
	readonly body: string;
};

/**
 * The banner, or null while any device answers or there are none. Offline
 * when the client knows it has no network; reconnecting while it tries a
 * device, or hasn't tried yet; else it shows what it saved. Each says when
 * a device last answered, at `now`, once any link knows.
 */
export const connectionBanner = (
	view: ViewState,
	options: { readonly online?: boolean; readonly now?: Timestamp } = {}
): ConnectionBanner | null => {
	const links = all(view, "device_link");
	// A link that waits for your answer reaches no device, even connected.
	if (links.length === 0 || links.some((l) => l.state.state === "connected" && !l.claim))
		return null;
	const seen = links.flatMap((l) =>
		l.state.state === "unreachable" && l.state.last_seen !== null ? [l.state.last_seen] : []
	);
	const saved =
		seen.length > 0 && options.now !== undefined
			? t("Last answered {{formatAgo_Math_max____seen___options_now}}.", {
					formatAgo_Math_max____seen___options_now: formatAgo(
						Math.max(...seen),
						options.now
					),
				})
			: t("What you see was saved when it last answered.");
	if (options.online === false) {
		return {
			tone: "offline",
			title: t("You are offline"),
			body: t("{{saved}} MaxHQ reconnects once you're back.", { saved: saved }),
		};
	}
	const dialing = links.filter((l) => l.state.state !== "unreachable");
	if (dialing.length > 0) {
		const title =
			dialing.length === 1
				? t("Reconnecting to {{dialing_0___name}}", { dialing_0___name: dialing[0]!.name })
				: t("Reconnecting to {{plural_dialing_length___device}}", {
						plural_dialing_length___device: plural(dialing.length, "device"),
					});
		return { tone: "reconnecting", title, body: saved };
	}
	return {
		tone: "cached",
		title:
			links.length === 1
				? t("Can't reach {{links_0___name}}", { links_0___name: links[0]!.name })
				: t("Can't reach your devices"),
		body: t("{{saved}} MaxHQ keeps trying in the background.", { saved: saved }),
	};
};
