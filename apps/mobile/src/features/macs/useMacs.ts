// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Macs the phone's core says it pairs with. They're read again when
// the view's links change, which a Mac's new book does, after pairing or
// forgetting, and when Settings opens. Screens read them, with this
// phone's choice of how they show, through `useMacs`.

import { useEffect, useMemo } from "react";
import { createStore, useStore } from "zustand";

import type { ViewState } from "@maxhq/shared/view/replica";

import type { PairedMac, PhoneCalls } from "../../data/native-source";
import { useServices, useView } from "../../services";
import { type MacPlaces, macPlaces, type MacsMode, macsMode, macView } from "./macs";
import { macsPref } from "./pref";

type Paired = {
	readonly macs: readonly PairedMac[];
	/** As JSON, to tell when a read changed nothing. */
	readonly kept: string;
};

const paired = createStore<Paired>(() => ({ macs: [], kept: "[]" }));

/** Reads the Macs from the core again. The screens redraw only when they changed. */
export const refreshMacs = (phone: PhoneCalls | null) => {
	if (!phone) return;
	const reply = phone.pairedMacs();
	if (reply.status !== "ok") return;
	const kept = JSON.stringify(reply.data);
	if (kept !== paired.getState().kept) paired.setState({ macs: reply.data, kept });
};

/** The view's links by ID and name, which change when a Mac's book does. */
const linksOf = (view: ViewState): string =>
	[...view.tables.device_link.values()]
		.map((link) => `${link.id}=${link.name}`)
		.sort()
		.join("\n");

/** The paired Macs, which Mac each device belongs to, and how the agent list shows them. */
export type Macs = {
	readonly macs: readonly PairedMac[];
	readonly places: MacPlaces;
	readonly mode: MacsMode;
};

export const useMacs = (): Macs => {
	const { phone } = useServices();
	const links = useView((s) => linksOf(s.view));
	useEffect(() => refreshMacs(phone), [phone, links]);
	const macs = useStore(paired, (s) => s.macs);
	const pref = useStore(macsPref);
	return useMemo(
		() => ({ macs, places: macPlaces(macs), mode: macsMode(macs, pref) }),
		[macs, pref]
	);
};

/** `view` with only the picked Mac's, while the Macs show one at a time. */
export const useMacView = (view: ViewState, { places, mode }: Macs): ViewState =>
	useMemo(
		() => (mode.kind === "one" ? macView(view, places, mode.picked) : view),
		[view, places, mode]
	);
