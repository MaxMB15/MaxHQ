// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The question MaxHQ asks when a link's hello names a device another link
// already knows: the card's words and the details to judge it by, what a
// held link's dot says meanwhile, and the note a link keeps once you say
// it's a different device. A held link reaches no device, so nothing meant
// for the device goes through it until you answer.

import { t } from "../i18n";

import type { ClaimedLink, DeviceLink, LinkClaimDetails, Timestamp } from "../bindings";
import type { Fact } from "../status/card";
import type { Connection } from "../status/connection";
import { formatWhen } from "../status/format";
import { all, type ViewState } from "../view/replica";

/** Where you change your answer later, as the app says it too. */
export const WHERE_TO_CHANGE = "Identity, on the device's page in the sidebar";

/** The names the question uses. */
export type ClaimNames = {
	/** Your name for the link that waits. */
	readonly link: string;
	/** The device its hello named. */
	readonly device: string;
	/** The link that knows the device already, when one does. */
	readonly knownBy: string | null;
	/** Whether you said it's a different device. */
	readonly different: boolean;
};

/** The names for `link`'s question, or null when it isn't held. */
export const claimNames = (view: ViewState, link: DeviceLink): ClaimNames | null => {
	const claim = link.claim;
	if (!claim) return null;
	const known = claim.known_by === null ? undefined : view.tables.device_link.get(claim.known_by);
	const reaching = all(view, "device_link").find((l) => l.device === claim.device && !l.claim);
	const device =
		reaching?.name ??
		view.tables.device.get(claim.device)?.name ??
		known?.name ??
		t("another device");
	return {
		link: link.name,
		device,
		knownBy: known?.name ?? null,
		different: claim.state === "different",
	};
};

/** Whether nothing goes through `link` until you answer about its device. */
export const isHeld = (link: DeviceLink): boolean => !!link.claim;

/** The short word a device row shows for a held link, or null when it isn't. */
export const heldLabel = (link: DeviceLink): string | null =>
	!link.claim ? null : link.claim.state === "asking" ? t("Needs your answer") : t("Cut off");

/** One side of the question, with the details to judge it by. */
export type ClaimSide = {
	readonly title: string;
	readonly facts: readonly Fact[];
};

/** The card that asks whether a link is the device its hello named. */
export type ClaimCard = {
	readonly title: string;
	/** The exact risk, in a sentence or two. */
	readonly risk: string;
	/** The link that waits, then the one that knows the device. */
	readonly sides: readonly ClaimSide[];
	/** The answer buttons. */
	readonly same: string;
	readonly different: string;
	/** Where the answer is kept and changed. */
	readonly remember: string;
};

/** The card's title. */
export const claimTitle = (names: ClaimNames): string =>
	names.knownBy === null
		? t("{{names_link}} says it is {{names_device}}, whose host keys MaxHQ already keeps", {
				names_link: names.link,
				names_device: names.device,
			})
		: t("{{names_link}} says it is {{names_device}}, which {{names_knownBy}} already knows", {
				names_link: names.link,
				names_device: names.device,
				names_knownBy: names.knownBy,
			});

/** What could go wrong if you say it's the same device and it isn't. */
export const claimRisk = (names: ClaimNames): string =>
	t(
		"If it isn't really {{names_device}}, it could receive sign-in tokens and actions meant for ",
		{ names_device: names.device }
	) +
	t(
		"{{names_device}}, and add its keys under {{names_device_1}}'s name. Until you answer, nothing ",
		{ names_device: names.device, names_device_1: names.device }
	) +
	t("goes through it.");

const list = (values: readonly string[], none: string): string =>
	values.length === 0 ? none : values.join(", ");

const sideFacts = (side: ClaimedLink, now: Timestamp): Fact[] => [
	{
		label: side.addresses.length > 1 ? t("Addresses") : t("Address"),
		value: list(side.addresses, t("None")),
	},
	{
		label: side.fingerprints.length > 1 ? t("Host keys") : t("Host key"),
		value: list(side.fingerprints, t("Not known")),
	},
	{
		label: t("First seen"),
		value: side.first_seen === null ? t("Not known") : formatWhen(side.first_seen, now),
	},
];

/** The whole card, with the details once the app has sent them. */
export const claimCard = (
	names: ClaimNames,
	details: LinkClaimDetails | null,
	now: Timestamp
): ClaimCard => {
	const sides: ClaimSide[] = [];
	if (details) {
		sides.push({ title: names.link, facts: sideFacts(details.link, now) });
		if (details.known_by) {
			sides.push({ title: details.known_by.name, facts: sideFacts(details.known_by, now) });
		}
		if (details.device_keys.length > 0) {
			sides.push({
				title: t("Kept for {{names_device}}", { names_device: names.device }),
				facts: [
					{
						label: details.device_keys.length > 1 ? t("Host keys") : t("Host key"),
						value: details.device_keys.join(", "),
					},
				],
			});
		}
	}
	const where = details?.where_to_change ?? WHERE_TO_CHANGE;
	return {
		title: claimTitle(names),
		risk: claimRisk(names),
		sides,
		same: t("Same device"),
		different: t("Different device"),
		remember: t("MaxHQ keeps your answer. You can change it under {{where}}.", {
			where: where,
		}),
	};
};

/** The note a link keeps once you said it's a different device. */
export const differentNote = (names: ClaimNames): string =>
	t("You said {{names_link}} isn't {{names_device}}, so nothing goes through it. ", {
		names_link: names.link,
		names_device: names.device,
	}) + t("Ask again if that changes.");

/** What forgetting a link's identity does, by its button. */
export const FORGET_IDENTITY = "Forget this device's identity";

/** The line by the forget button. */
export const forgetNote = (link: string, device: string): string =>
	t("{{link}} reaches {{device}}. If you forget that, MaxHQ asks you at its next hello ", {
		link: link,
		device: device,
	}) + t("when another link knows the device it names.");

/** A held link's dot and hover, or undefined when it isn't held. */
export const heldConnection = (
	link: DeviceLink,
	names: ClaimNames | undefined
): Connection | undefined => {
	if (!link.claim) return undefined;
	const device = names?.device ?? t("a device you have");
	if (link.claim.state === "different") {
		return {
			look: "unreachable",
			card: {
				title: t("{{link_name}} is cut off", { link_name: link.name }),
				body: t("You said it isn't {{device}}, so nothing goes through it.", {
					device: device,
				}),
				facts: [],
				foot: t("Ask again under {{WHERE_TO_CHANGE}}", {
					WHERE_TO_CHANGE: WHERE_TO_CHANGE,
				}),
			},
		};
	}
	return {
		look: "connecting",
		card: {
			title: t("{{link_name}} waits for your answer", { link_name: link.name }),
			body: t(
				"It says it is {{device}}. Nothing goes through it until you say whether it is.",
				{ device: device }
			),
			facts: [],
			foot: t("Answer on the Mac, under {{WHERE_TO_CHANGE}}", {
				WHERE_TO_CHANGE: WHERE_TO_CHANGE,
			}),
		},
	};
};
