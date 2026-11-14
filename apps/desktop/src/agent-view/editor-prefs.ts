// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which editor this Mac's Open in buttons use: the last one picked, and the
// default editor setting, which can fix them on one editor or hide them.
// Both live in localStorage, since editors belong to this Mac, and every
// window takes up the others' changes.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import {
	type DefaultEditor,
	readDefaultEditor,
	writeDefaultEditor,
} from "@maxhq/shared/editor/open";
import { t as translate } from "@maxhq/shared/i18n";

/** The storage key for the last editor used. */
export const LAST_EDITOR_KEY = "maxhq.editor";

/** The storage key for the default editor setting. */
export const DEFAULT_EDITOR_KEY = "maxhq.editor.default";

/** The editors this Mac's settings say to use. */
type EditorPrefs = { readonly last: string | null; readonly fixed: DefaultEditor };

const load = (key: string): string | null => {
	try {
		return localStorage.getItem(key);
	} catch {
		return null;
	}
};

const save = (key: string, value: string, what: string): void => {
	try {
		localStorage.setItem(key, value);
	} catch (error) {
		console.warn(translate("Couldn't save {{what}}", { what: what }), error);
	}
};

const stores = new WeakMap<Services, StoreApi<EditorPrefs>>();

/** The editor settings for one window, read from storage the first time. */
export const editorPrefsOf = (services: Services): StoreApi<EditorPrefs> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<EditorPrefs>(() => ({
			last: load(LAST_EDITOR_KEY),
			fixed: readDefaultEditor(load(DEFAULT_EDITOR_KEY)),
		}));
		stores.set(services, store);
	}
	return store;
};

/** Records the editor a pick opened. */
export const setLast = (services: Services, editor: string): void => {
	editorPrefsOf(services).setState({ last: editor });
	save(LAST_EDITOR_KEY, editor, translate("the editor you used"));
};

/** Changes the default editor setting and saves it. */
export const setDefaultEditor = (services: Services, fixed: DefaultEditor): void => {
	editorPrefsOf(services).setState({ fixed });
	save(DEFAULT_EDITOR_KEY, writeDefaultEditor(fixed), translate("the default editor"));
};

/** Reads the default editor setting. */
export const useDefaultEditor = (): DefaultEditor =>
	useStore(editorPrefsOf(useServices()), (s) => s.fixed);

/**
 * Takes up the editors another window saves, so every window's buttons
 * agree. Returns a function that stops.
 */
export const watchEditorPrefs = (services: Services): (() => void) => {
	const onStorage = (e: StorageEvent) => {
		if (e.key === LAST_EDITOR_KEY) editorPrefsOf(services).setState({ last: e.newValue });
		if (e.key === DEFAULT_EDITOR_KEY)
			editorPrefsOf(services).setState({ fixed: readDefaultEditor(e.newValue) });
	};
	window.addEventListener("storage", onStorage);
	return () => window.removeEventListener("storage", onStorage);
};
