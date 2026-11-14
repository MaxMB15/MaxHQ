// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { createInstance, type TOptions } from "i18next";
import { useMemo, useSyncExternalStore } from "react";
import { initReactI18next } from "react-i18next";
import enShared from "./i18n/locales/en/shared.json";
import enDesktop from "./i18n/locales/en/desktop.json";
import enSettings from "./i18n/locales/en/settings.json";
import enMobile from "./i18n/locales/en/mobile.json";
import nlShared from "./i18n/locales/nl/shared.json";
import nlDesktop from "./i18n/locales/nl/desktop.json";
import nlSettings from "./i18n/locales/nl/settings.json";
import nlMobile from "./i18n/locales/nl/mobile.json";
import deShared from "./i18n/locales/de/shared.json";
import deDesktop from "./i18n/locales/de/desktop.json";
import deSettings from "./i18n/locales/de/settings.json";
import deMobile from "./i18n/locales/de/mobile.json";
import frShared from "./i18n/locales/fr/shared.json";
import frDesktop from "./i18n/locales/fr/desktop.json";
import frSettings from "./i18n/locales/fr/settings.json";
import frMobile from "./i18n/locales/fr/mobile.json";
import esShared from "./i18n/locales/es/shared.json";
import esDesktop from "./i18n/locales/es/desktop.json";
import esSettings from "./i18n/locales/es/settings.json";
import esMobile from "./i18n/locales/es/mobile.json";

export const LANGUAGES = ["en", "nl", "de", "fr", "es"] as const;
export type Language = (typeof LANGUAGES)[number];
export type LanguageChoice = Language | "system";
export const LANGUAGE_KEY = "maxhq.interface-language";

export const languageChoiceOf = (value: unknown): LanguageChoice =>
	typeof value === "string" && (value === "system" || LANGUAGES.includes(value as Language))
		? (value as LanguageChoice)
		: "system";

export const languageFor = (choice: unknown, systemLanguages: readonly string[]): Language => {
	const selected = languageChoiceOf(choice);
	if (selected !== "system") return selected;
	const base = systemLanguages[0]?.replace(/_/g, "-").split("-")[0]?.toLowerCase();
	return LANGUAGES.includes(base as Language) ? (base as Language) : "en";
};

export const i18n = createInstance();
void i18n.use(initReactI18next).init({
	lng: "en",
	fallbackLng: "en",
	supportedLngs: [...LANGUAGES],
	initAsync: false,
	keySeparator: false,
	nsSeparator: false,
	interpolation: { escapeValue: false },
	resources: {
		en: { translation: { ...enShared, ...enDesktop, ...enSettings, ...enMobile } },
		nl: { translation: { ...nlShared, ...nlDesktop, ...nlSettings, ...nlMobile } },
		de: { translation: { ...deShared, ...deDesktop, ...deSettings, ...deMobile } },
		fr: { translation: { ...frShared, ...frDesktop, ...frSettings, ...frMobile } },
		es: { translation: { ...esShared, ...esDesktop, ...esSettings, ...esMobile } },
	},
});

export const t = (key: string, options?: TOptions): string => {
	const values = Object.fromEntries(
		Object.entries(options ?? {}).map(([name, value]) => [
			name,
			name !== "count" && typeof value === "number" && !key.includes(`{{${name},`)
				? formatNumber(value)
				: value,
		])
	);
	return String(i18n.t(key, values));
};

export type LanguagePreferences = {
	readonly read: () => string | null;
	readonly write: (choice: LanguageChoice) => void;
	readonly systemLanguages: () => readonly string[];
};
let preferences: LanguagePreferences = {
	read: () => null,
	write: () => {},
	systemLanguages: () => ["en"],
};
let choice: LanguageChoice = "system";
const listeners = new Set<() => void>();
const notify = () => {
	for (const listener of listeners) listener();
};
i18n.on("languageChanged", notify);
const subscribe = (listener: () => void) => {
	listeners.add(listener);
	return () => {
		listeners.delete(listener);
	};
};
const snapshot = () => `${choice}:${i18n.resolvedLanguage ?? i18n.language}`;

const apply = () => {
	let system: readonly string[] = [];
	try {
		system = preferences.systemLanguages();
	} catch {
		/* Use English when the platform has no locale. */
	}
	void i18n.changeLanguage(languageFor(choice, system));
	notify();
};

export const configureLanguage = (adapter: LanguagePreferences): (() => void) => {
	preferences = adapter;
	try {
		choice = languageChoiceOf(adapter.read());
	} catch {
		choice = "system";
	}
	apply();
	return () => {
		if (preferences === adapter)
			preferences = { read: () => null, write: () => {}, systemLanguages: () => ["en"] };
	};
};

export const setLanguageChoice = (value: LanguageChoice): void => {
	choice = languageChoiceOf(value);
	try {
		preferences.write(choice);
	} catch {
		/* Keep the choice for this session. */
	}
	apply();
};

export const refreshLanguage = (): void => apply();
export const useI18n = () => {
	const revision = useSyncExternalStore(subscribe, snapshot, snapshot);
	const translate = useMemo(() => {
		void revision;
		return (key: string, options?: TOptions) => t(key, options);
	}, [revision]);
	return {
		t: translate,
		language: (i18n.language ?? "en") as Language,
		choice,
		setLanguageChoice,
	};
};

const locale = () => i18n.language ?? "en";
export const formatNumber = (value: number, options?: Intl.NumberFormatOptions): string =>
	new Intl.NumberFormat(locale(), options).format(value);
export const formatDate = (
	value: Date | number | string,
	options?: Intl.DateTimeFormatOptions
): string =>
	new Intl.DateTimeFormat(locale(), options).format(
		typeof value === "string" ? new Date(value) : value
	);
export const formatRelativeTime = (
	value: number,
	unit: Intl.RelativeTimeFormatUnit,
	options?: Intl.RelativeTimeFormatOptions
): string => new Intl.RelativeTimeFormat(locale(), options).format(value, unit);
export const formatList = (values: readonly string[], options?: Intl.ListFormatOptions): string =>
	new Intl.ListFormat(locale() === "en" ? "en-GB" : locale(), options).format(values);
