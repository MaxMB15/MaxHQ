// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether the window's settings are open, and where. Settings is one page
// you scroll, with a tab for each device and one for phones, and a few
// views of their own that show in place of the page with a Back button.
// The header's button and Cmd+, open it. Nothing a form holds lives here,
// so a secret typed into one never reaches this store.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import { shortcutListOf } from "@/shortcut-list/store";
import type { DeviceId } from "@maxhq/shared/bindings";
import {
	type DeviceCardId,
	type PageCard,
	type PhoneCard,
	SETTINGS_GROUPS,
	type SettingsDetail,
	type SettingsGroup,
	type SettingsPlace,
	type SettingsProjectTab,
} from "@maxhq/shared/guide/settings-link";
import type { LoginPage as Login } from "@maxhq/shared/settings/login-steps";

/** A login the settings walk you through. */
export type LoginPage = Login;

/** A tab of a project's settings. */
export type ProjectTab = SettingsProjectTab;

/** The tab the Devices and phones group shows: a device, Phones, or this Mac. */
export type DeviceTab = DeviceId | "phones" | null;

/** The settings' state. */
export type SettingsState = {
	readonly open: boolean;
	/** Counts each opening, so the forms start fresh every time. */
	readonly opened: number;
	/** The view of its own that shows in place of the page, if any. */
	readonly detail: SettingsDetail | null;
	/** The Devices and phones group's tab. */
	readonly tab: DeviceTab;
	/** The element the page scrolls to once it shows, then forgets. */
	readonly reveal: string | null;
};

const stores = new WeakMap<Services, StoreApi<SettingsState>>();

// Where the page was scrolled, so Back and a new opening of the same
// place come back to it. Kept off the state, since only the page reads it.
const scrolls = new WeakMap<Services, number>();

/** The settings' store for one window. */
export const settingsOf = (services: Services): StoreApi<SettingsState> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<SettingsState>(() => ({
			open: false,
			opened: 0,
			detail: null,
			tab: null,
			reveal: null,
		}));
		stores.set(services, store);
	}
	return store;
};

/** The element ID of a group on the page. */
export const groupId = (group: SettingsGroup): string => `settings-group-${group}`;

/** The element ID of a card on the page. */
export const pageCardId = (card: PageCard): string => `settings-card-${card}`;

/** The element ID of a card on a device's tab or the Phones tab. Only one tab shows at a time. */
export const deviceCardId = (card: DeviceCardId | PhoneCard): string => `device-card-${card}`;

const isGroup = (at: string): at is SettingsGroup =>
	(SETTINGS_GROUPS as readonly string[]).includes(at);

// The state that shows a place, from the state before.
const placed = (place: SettingsPlace): Partial<SettingsState> => {
	switch (place.kind) {
		case "page":
			return {
				detail: null,
				reveal:
					place.at === null
						? null
						: isGroup(place.at)
							? groupId(place.at)
							: pageCardId(place.at),
			};
		case "device":
			return {
				detail: null,
				tab: place.device,
				reveal: place.card ? deviceCardId(place.card) : groupId("devices"),
			};
		case "phones":
			return {
				detail: null,
				tab: "phones",
				reveal: place.card ? deviceCardId(place.card) : groupId("devices"),
			};
		case "shortcuts":
			return {};
		default:
			return { detail: place };
	}
};

/**
 * Opens the settings at a place, or where you left them. The keyboard
 * shortcuts show over any view, so opening them closes the settings.
 */
export const openSettings = (services: Services, place?: SettingsPlace | null): void => {
	if (place?.kind === "shortcuts") {
		settingsOf(services).setState({ open: false });
		shortcutListOf(services).setState({ open: true });
		return;
	}
	const store = settingsOf(services);
	if (!store.getState().open && place) scrolls.delete(services);
	store.setState((s) => ({
		open: true,
		opened: s.opened + 1,
		...(place ? placed(place) : {}),
	}));
};

/** Shows another place while the settings are open. */
export const showPlace = (services: Services, place: SettingsPlace): void => {
	if (place.kind === "shortcuts") return openSettings(services, place);
	settingsOf(services).setState(placed(place));
};

/** Leaves a view of its own for the page, scrolled where it was. */
export const back = (services: Services): void => settingsOf(services).setState({ detail: null });

/** Closes the settings. */
export const closeSettings = (services: Services): void =>
	settingsOf(services).setState({ open: false });

/**
 * What Esc does: leaves a view of its own for the page first, and closes
 * the settings from the page. True when it went back, so the window stays.
 */
export const escapeSettings = (services: Services): boolean => {
	if (settingsOf(services).getState().detail === null) {
		closeSettings(services);
		return false;
	}
	back(services);
	return true;
};

/**
 * Opens the settings on a device's tab, scrolled to one of its cards, or
 * at the tab's top with none.
 */
export const showDeviceCard = (
	services: Services,
	device: DeviceId,
	card: DeviceCardId | null
): void => openSettings(services, { kind: "device", device, card });

/** Forgets the element to scroll to, once the page has. */
export const revealed = (services: Services): void =>
	settingsOf(services).setState({ reveal: null });

/** Remembers where the page is scrolled. */
export const keepScroll = (services: Services, top: number): void => {
	scrolls.set(services, top);
};

/** Where the page was scrolled, or its top. */
export const scrollOf = (services: Services): number => scrolls.get(services) ?? 0;

/** Reads the settings' state. */
export const useSettings = <T>(selector: (state: SettingsState) => T): T =>
	useStore(settingsOf(useServices()), selector);
