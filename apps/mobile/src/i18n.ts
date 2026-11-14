// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import "./intl";

import { File, Paths } from "expo-file-system";
import { getLocales } from "expo-localization";
import { AppState } from "react-native";

import { configureLanguage, LANGUAGE_KEY, refreshLanguage } from "@maxhq/shared/i18n";

const kept = () => new File(Paths.document, "language.json");

configureLanguage({
	read: () => {
		try {
			const file = kept();
			if (!file.exists) return null;
			const saved: unknown = JSON.parse(file.textSync());
			if (typeof saved !== "object" || saved === null) return null;
			const choice = (saved as Record<string, unknown>)[LANGUAGE_KEY];
			return typeof choice === "string" ? choice : null;
		} catch {
			return null;
		}
	},
	write: (choice) => {
		try {
			kept().write(JSON.stringify({ [LANGUAGE_KEY]: choice }));
		} catch {
			// Keep the choice for this run when the file cannot be written.
		}
	},
	systemLanguages: () => getLocales().map((locale) => locale.languageTag),
});

AppState.addEventListener("change", (state) => {
	if (state === "active") refreshLanguage();
});
