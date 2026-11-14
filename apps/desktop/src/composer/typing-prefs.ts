// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The typing settings, Check spelling and Correct spelling automatically,
// and what they make of the boxes you type prompts and notes in. They live
// in localStorage, since spelling belongs to this Mac, and every window
// takes up the others' changes.

import { useEffect } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServicesIfAny } from "@/app/services";
import {
	readTypingPrefs,
	type SpellingMode,
	spellingMode,
	type TypingAttributes,
	typingAttributes,
	type TypingPrefs,
	writeTypingPrefs,
} from "@maxhq/shared/editor/typing";
import { t as translate } from "@maxhq/shared/i18n";
import type { Editor } from "@tiptap/core";

import { type ComposerNative, useComposerNative } from "./native";
import { switchSpelling } from "./spelling";

/** The storage key. */
export const TYPING_KEY = "maxhq.typing";

const load = (): string | null => {
	try {
		return localStorage.getItem(TYPING_KEY);
	} catch {
		return null;
	}
};

const stores = new WeakMap<Services, StoreApi<TypingPrefs>>();

/** The typing settings for one window, read from storage the first time. */
export const typingPrefsOf = (services: Services): StoreApi<TypingPrefs> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<TypingPrefs>(() => readTypingPrefs(load()));
		stores.set(services, store);
	}
	return store;
};

/** Changes the typing settings and saves them. */
export const setTypingPrefs = (services: Services, change: Partial<TypingPrefs>): void => {
	const store = typingPrefsOf(services);
	store.setState(change);
	try {
		localStorage.setItem(TYPING_KEY, writeTypingPrefs(store.getState()));
	} catch (error) {
		console.warn(
			translate("Couldn't save {{what}}", { what: translate("the typing settings") }),
			error
		);
	}
};

// An editor on its own, as in a test, reads the saved settings once.
let loose: StoreApi<TypingPrefs> | null = null;
const looseStore = (): StoreApi<TypingPrefs> =>
	(loose ??= createStore<TypingPrefs>(() => readTypingPrefs(load())));

/** Reads the typing settings. */
export const useTypingPrefs = (): TypingPrefs => {
	const services = useServicesIfAny();
	return useStore(services ? typingPrefsOf(services) : looseStore());
};

/** Takes up the settings another window saves. Returns a function that stops. */
export const watchTypingPrefs = (services: Services): (() => void) => {
	const onStorage = (e: StorageEvent) => {
		if (e.key === TYPING_KEY) typingPrefsOf(services).setState(readTypingPrefs(e.newValue));
	};
	window.addEventListener("storage", onStorage);
	return () => window.removeEventListener("storage", onStorage);
};

/** How a prompt or note editor types: who checks its spelling, and its attributes. */
export type EditorTyping = { readonly mode: SpellingMode; readonly attributes: TypingAttributes };

/** The typing a rich editor gets from the settings. */
export const useEditorTyping = (native: ComposerNative): EditorTyping => {
	const prefs = useTypingPrefs();
	const mode = spellingMode(prefs, { nativeSpelling: native.hasNativeSpelling(), rich: true });
	return { mode, attributes: typingAttributes(prefs, mode) };
};

/** Turns an editor's own spell check on or off as the settings change. */
export const useSpellingSwitch = (editor: Editor, mode: SpellingMode): void => {
	useEffect(() => {
		if (!editor.isDestroyed) switchSpelling(editor.view, mode === "native");
	}, [editor, mode]);
};

/** The typing props a plain input or text area takes, as React writes them. */
export type FieldTyping = {
	readonly spellCheck: boolean;
	readonly autoCorrect: "on" | "off";
	readonly autoCapitalize: "off";
};

const fieldTypingOf = (prefs: TypingPrefs, nativeSpelling: boolean): FieldTyping => {
	const attributes = typingAttributes(
		prefs,
		spellingMode(prefs, { nativeSpelling, rich: false })
	);
	return {
		spellCheck: attributes.spellcheck === "true",
		autoCorrect: attributes.autocorrect,
		autoCapitalize: attributes.autocapitalize,
	};
};

/** The typing props for a plain input or text area. */
export const useFieldTyping = (): FieldTyping =>
	fieldTypingOf(useTypingPrefs(), useComposerNative().hasNativeSpelling());

/**
 * Gives the whole page a plain field's typing, which every input without its
 * own `spellcheck` takes up, as the settings' fields do. With spell check
 * off WebKit doesn't autocorrect either. Returns a function that stops.
 */
export const watchDocumentTyping = (services: Services, nativeSpelling: boolean): (() => void) => {
	const root = document.documentElement;
	const store = typingPrefsOf(services);
	const apply = () => {
		const typing = fieldTypingOf(store.getState(), nativeSpelling);
		root.setAttribute("spellcheck", String(typing.spellCheck));
		root.setAttribute("autocorrect", typing.autoCorrect);
		root.setAttribute("autocapitalize", typing.autoCapitalize);
	};
	apply();
	const stop = store.subscribe(apply);
	return () => {
		stop();
		for (const name of ["spellcheck", "autocorrect", "autocapitalize"])
			root.removeAttribute(name);
	};
};
