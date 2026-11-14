// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a device's settings say about its links to the other devices: where
// each stands, and the one button its row offers. Remove turns the pair
// off, and Allow turns it back on.

import { t } from "../i18n";

import type { PeerLink, PeerLinkState } from "../bindings";

/** Where a link stands, in words. */
export const LINK_STATES: Record<PeerLinkState, string> = {
	get linked() {
		return t("Linked");
	},
	get waiting() {
		return t("Waiting for the device");
	},
	get off() {
		return t("Off");
	},
};

/** What a link's row says under its name: why it can't link yet, or where it stands. */
export const linkSub = (link: PeerLink): string => link.note ?? LINK_STATES[link.state];

/** The button a link's row offers, and whether it turns the link on. */
export const linkAction = (link: PeerLink): { label: string; on: boolean } =>
	link.state === "off" ? { label: t("Allow"), on: true } : { label: t("Remove"), on: false };
