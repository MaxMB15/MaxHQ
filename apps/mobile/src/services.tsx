// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Hands the data source and the view store to every screen, and keeps the
// view current and the drafts saved while the app runs. On the phone's
// core, it also tells the core when the app comes to the screen or leaves
// it, and when the phone changes networks.

import NetInfo from "@react-native-community/netinfo";
import { createContext, type ReactNode, useContext, useEffect, useState } from "react";
import { AppState } from "react-native";
import { useStore } from "zustand";

import type { DataSource } from "@maxhq/shared/data/source";
import { createNetworkTeller } from "@maxhq/shared/status/phone-network";
import {
	connectView,
	createViewStore,
	type ViewStore,
	type ViewStoreState,
} from "@maxhq/shared/view/store";

import type { PhoneCalls } from "./data/native-source";
import { watchDrafts } from "./features/agent/saved-drafts";

/** What every screen reaches through `useServices`. */
export type Services = {
	/** Where the view comes from and where commands go. */
	readonly source: DataSource;
	/** The view. */
	readonly view: ViewStore;
	/** The phone's own calls, when the source is its core. */
	readonly phone: PhoneCalls | null;
};

const ServicesContext = createContext<Services | null>(null);

// Only the network's kind is read, so NetInfo needn't check the internet
// by fetching a web page. This runs before any screen listens.
NetInfo.configure({ reachabilityShouldRun: () => false });

/** Starts the view store for a source and keeps it running while mounted. */
export const ServicesProvider = ({
	source,
	phone,
	children,
}: {
	source: DataSource;
	phone: PhoneCalls | null;
	children: ReactNode;
}) => {
	const [services] = useState<Services>(() => ({ source, phone, view: createViewStore() }));
	useEffect(() => connectView(services.view, services.source), [services]);
	useEffect(() => watchDrafts(services.view), [services]);
	useEffect(() => {
		const { phone } = services;
		if (!phone) return;
		const network = createNetworkTeller(
			(kind) => phone.networkChanged(kind),
			AppState.currentState === "active"
		);
		const watch = AppState.addEventListener("change", (state) => {
			if (state === "active" || state === "background") phone.appState(state);
			network.active(state === "active");
		});
		const stop = NetInfo.addEventListener(network.network);
		return () => {
			watch.remove();
			stop();
		};
	}, [services]);
	return <ServicesContext value={services}>{children}</ServicesContext>;
};

/** The source and store from the nearest `ServicesProvider`. */
export const useServices = (): Services => {
	const services = useContext(ServicesContext);
	if (!services) throw new Error("useServices needs a ServicesProvider above it");
	return services;
};

/** Reads from the view store. Select a stable value, such as `s.view`, then derive with `useMemo`. */
export const useView = <T,>(selector: (state: ViewStoreState) => T): T =>
	useStore(useServices().view, selector);

/** Whether the app is in front, rather than in the background or behind a system sheet. */
export const useAppActive = (): boolean => {
	const [active, setActive] = useState(() => AppState.currentState === "active");
	useEffect(() => {
		const watch = AppState.addEventListener("change", (state) => setActive(state === "active"));
		return () => watch.remove();
	}, []);
	return active;
};
