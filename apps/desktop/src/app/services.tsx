// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Hands the data source and the two stores to every component, and runs the
// work that lives as long as the window: keeping the view current, opening
// an agent when a notification asks, telling the backend what the panes
// show, noticing auto-settles, saving which shelves are open, keeping the
// drafts saved, taking up sidebar, editor, typing and dispatcher settings
// other windows save, giving every plain field the typing settings, handing
// a run to its lead once one binds it, and keeping the Dock badge's count.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createContext, type ReactNode, useContext, useEffect, useState } from "react";
import { useStore } from "zustand";

import type { DataSource } from "@maxhq/shared/data/source";
import { watchEditorPrefs } from "@/agent-view/editor-prefs";
import { useComposerNative } from "@/composer/native";
import { watchDocumentTyping, watchTypingPrefs } from "@/composer/typing-prefs";
import { watchDrafts } from "@/composer/saved-drafts";
import { watchDispatcherPrefs } from "@/header/dispatcher-prefs";
import { watchShown } from "@/layout/shown";
import { watchDockBadge } from "@/native/dock-badge";
import { noticesOf } from "@/sidebar/notices";
import { loadShelves, watchShelves } from "@/sidebar/open-shelves";
import { watchPrefs } from "@/sidebar/prefs";
import { watchAutoSettles } from "@maxhq/shared/ui/inbox-notice";
import { watchHandOver } from "@maxhq/shared/workflow/hand-over";
import { type UiActions, type UiState, type UiStore, createUiStore } from "@maxhq/shared/ui/store";
import {
	connectView,
	createViewStore,
	type ViewStore,
	type ViewStoreState,
} from "@maxhq/shared/view/store";
import { openAgent } from "./open-agent";

/** What every component reaches through `useServices`. */
export type Services = {
	/** Where the view comes from and where commands go. */
	readonly source: DataSource;
	/** The view. */
	readonly view: ViewStore;
	/** The window's own state. */
	readonly ui: UiStore;
};

const ServicesContext = createContext<Services | null>(null);

/** Starts the stores for a source and keeps them running while mounted. */
export const ServicesProvider = ({
	source,
	ui,
	children,
}: {
	source: DataSource;
	/** A store to start from, for tests. */
	ui?: UiStore;
	children: ReactNode;
}) => {
	const [services] = useState<Services>(() => ({
		source,
		view: createViewStore(),
		ui: ui ?? createUiStore({ openShelves: loadShelves() }),
	}));
	const [queries] = useState(
		() => new QueryClient({ defaultOptions: { queries: { retry: false } } })
	);

	useEffect(() => connectView(services.view, services.source), [services]);

	useEffect(() => {
		let stopped = false;
		let stop: (() => void) | undefined;
		services.source
			.onFocusAgent((agent) => openAgent(services, agent))
			.then((unsubscribe) => {
				if (stopped) unsubscribe();
				else stop = unsubscribe;
			}, console.error);
		return () => {
			stopped = true;
			stop?.();
		};
	}, [services]);

	useEffect(() => watchShown(services), [services]);

	useEffect(() => watchAutoSettles(noticesOf(services)), [services]);

	useEffect(() => watchPrefs(services), [services]);

	useEffect(() => watchEditorPrefs(services), [services]);

	useEffect(() => watchTypingPrefs(services), [services]);

	const nativeSpelling = useComposerNative().hasNativeSpelling();
	useEffect(() => watchDocumentTyping(services, nativeSpelling), [services, nativeSpelling]);

	useEffect(() => watchDispatcherPrefs(services), [services]);

	useEffect(() => watchShelves(services), [services]);

	useEffect(() => watchDrafts(services), [services]);

	useEffect(() => watchHandOver(services.view, services.ui), [services]);

	useEffect(() => watchDockBadge(services), [services]);

	return (
		<QueryClientProvider client={queries}>
			<ServicesContext value={services}>{children}</ServicesContext>
		</QueryClientProvider>
	);
};

/** The source and stores from the nearest `ServicesProvider`. */
export const useServices = (): Services => {
	const services = useContext(ServicesContext);
	if (!services) throw new Error("useServices needs a ServicesProvider above it");
	return services;
};

/** The services, or null outside a `ServicesProvider`, such as an editor on its own in a test. */
export const useServicesIfAny = (): Services | null => useContext(ServicesContext);

/** Reads from the view store. Select a stable value, such as `s.view`, then derive with `useMemo`. */
export const useView = <T,>(selector: (state: ViewStoreState) => T): T =>
	useStore(useServices().view, selector);

/** Reads from the window's store. */
export const useUi = <T,>(selector: (state: UiState & UiActions) => T): T =>
	useStore(useServices().ui, selector);
