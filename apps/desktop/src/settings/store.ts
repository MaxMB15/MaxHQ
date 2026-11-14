// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether the window's settings are open, and on which page. The header's
// button and Cmd+, open them. Nothing a form holds lives here, so a secret
// typed into one never reaches this store.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import type { DeviceId, LinkId, ProjectId } from "@maxhq/shared/bindings";
import { t as translate } from "@maxhq/shared/i18n";

/** A login the settings walk you through. */
export type LoginPage = "claude" | "codex" | "cursor" | "github" | "apns";

/** A tab of a project's page. */
export type ProjectTab = "general" | "skills";

/** A page in the settings. */
export type SettingsPage =
	| {
			readonly kind: "login";
			readonly login: LoginPage;
			/** The device a token or key form ticks alone, when one is given. */
			readonly device?: DeviceId;
	  }
	| {
			readonly kind: "project";
			readonly project: ProjectId;
			/** The tab it opens on, General when not given. */
			readonly tab?: ProjectTab;
	  }
	| { readonly kind: "appearance" }
	| { readonly kind: "sidebar" }
	| { readonly kind: "editor" }
	| { readonly kind: "dispatcher" }
	| { readonly kind: "agent-tools" }
	| { readonly kind: "scheduled-tasks" }
	| { readonly kind: "test-hosts" }
	| { readonly kind: "skills" }
	| { readonly kind: "notifications" }
	| { readonly kind: "phones" }
	| { readonly kind: "shortcuts" };

/** The settings' state. */
export type SettingsState = {
	readonly open: boolean;
	readonly page: SettingsPage;
	/** Counts each opening, so the forms start fresh every time. */
	readonly opened: number;
	/**
	 * The card a device's page scrolls to when it shows, after the search
	 * picked it there.
	 */
	readonly reveal: string | null;
};

// The first page of the App group, which the list shows first.
const FIRST: SettingsPage = { kind: "appearance" };

const stores = new WeakMap<Services, StoreApi<SettingsState>>();

/** The settings' store for one window. */
export const settingsOf = (services: Services): StoreApi<SettingsState> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<SettingsState>(() => ({
			open: false,
			page: FIRST,
			opened: 0,
			reveal: null,
		}));
		stores.set(services, store);
	}
	return store;
};

/** Opens the settings, on a page or on the one you left. */
export const openSettings = (services: Services, page?: SettingsPage): void =>
	settingsOf(services).setState((s) => ({
		open: true,
		page: page ?? s.page,
		opened: s.opened + 1,
	}));

/** Shows another page. */
export const showPage = (services: Services, page: SettingsPage): void =>
	settingsOf(services).setState({ page });

/** Closes the settings. */
export const closeSettings = (services: Services): void =>
	settingsOf(services).setState({ open: false });

/**
 * Closes the settings on a device's page, scrolled to one of its cards, or
 * at its top with none.
 */
export const showDeviceCard = (services: Services, link: LinkId, card: string | null): void => {
	services.ui.getState().selectLink(link);
	settingsOf(services).setState({ open: false, reveal: card });
};

/** Forgets the card to scroll to, once the device's page has. */
export const revealed = (services: Services): void =>
	settingsOf(services).setState({ reveal: null });

/** Whether two pages are the same one. */
export const samePage = (a: SettingsPage, b: SettingsPage): boolean => {
	if (a.kind === "login") return b.kind === "login" && a.login === b.login;
	if (a.kind === "project") return b.kind === "project" && a.project === b.project;
	return b.kind === a.kind;
};

/**
 * A name for the page that tells it from every other. A project's names
 * its tab too, so asking for another tab opens the page again on it.
 */
export const pageKey = (page: SettingsPage): string => {
	if (page.kind === "login") return `login:${page.login}`;
	if (page.kind === "project")
		return translate("project:{{project}}:{{value2}}", {
			project: page.project,
			value2: page.tab ?? "general",
		});
	return page.kind;
};

/** Reads the settings' state. */
export const useSettings = <T>(selector: (state: SettingsState) => T): T =>
	useStore(settingsOf(useServices()), selector);
