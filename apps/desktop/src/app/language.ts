// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { configureLanguage, i18n, LANGUAGE_KEY, refreshLanguage } from "@maxhq/shared/i18n";

/** Loads the interface language and follows changes in other windows. */
export const watchLanguage = (): (() => void) => {
	const adapter = {
		read: () => localStorage.getItem(LANGUAGE_KEY),
		write: (choice: string) => localStorage.setItem(LANGUAGE_KEY, choice),
		systemLanguages: () =>
			navigator.languages.length ? navigator.languages : [navigator.language],
	};
	const stop = configureLanguage(adapter);
	const paint = () => {
		document.documentElement.lang = i18n.resolvedLanguage ?? "en";
	};
	const onStorage = (event: StorageEvent) => {
		if (event.key === LANGUAGE_KEY || event.key === null) configureLanguage(adapter);
	};
	window.addEventListener("storage", onStorage);
	window.addEventListener("languagechange", refreshLanguage);
	i18n.on("languageChanged", paint);
	paint();
	return () => {
		stop();
		window.removeEventListener("storage", onStorage);
		window.removeEventListener("languagechange", refreshLanguage);
		i18n.off("languageChanged", paint);
	};
};
