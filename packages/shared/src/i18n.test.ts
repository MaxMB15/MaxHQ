// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it } from "vitest";
import {
	configureLanguage,
	formatDate,
	formatNumber,
	i18n,
	languageFor,
	setLanguageChoice,
	t,
} from "./i18n";

afterEach(() => {
	configureLanguage({ read: () => "en", write: () => {}, systemLanguages: () => ["en"] });
});

describe("interface language", () => {
	it("resolves regional supported system languages and otherwise falls back to English", () => {
		expect(languageFor("system", ["nl-NL"])).toBe("nl");
		expect(languageFor("system", ["de-AT"])).toBe("de");
		expect(languageFor("system", ["fr-CA"])).toBe("fr");
		expect(languageFor("system", ["es-MX"])).toBe("es");
		expect(languageFor("system", ["ja-JP", "nl"])).toBe("en");
		expect(languageFor("de", ["nl"])).toBe("de");
		expect(languageFor("invalid", ["es"])).toBe("es");
	});
	it("loads and persists a language choice independently of agent configuration", () => {
		const writes: string[] = [];
		configureLanguage({
			read: () => "nl",
			write: (choice) => writes.push(choice),
			systemLanguages: () => ["fr-FR"],
		});
		expect(i18n.language).toBe("nl");
		setLanguageChoice("de");
		expect(i18n.language).toBe("de");
		expect(writes).toEqual(["de"]);
		setLanguageChoice("system");
		expect(i18n.language).toBe("fr");
	});
	it("survives unavailable preference storage", () => {
		expect(() =>
			configureLanguage({
				read: () => {
					throw Error("unavailable");
				},
				write: () => {
					throw Error("unavailable");
				},
				systemLanguages: () => ["es"],
			})
		).not.toThrow();
		expect(i18n.language).toBe("es");
		expect(() => setLanguageChoice("en")).not.toThrow();
	});
	it("uses English fallback, interpolation and locale plurals", () => {
		i18n.addResourceBundle("en", "translation", {
			"test.fallback": "Hello {{name}}",
			"test.items_one": "{{count}} item",
			"test.items_other": "{{count}} items",
		});
		i18n.addResourceBundle("fr", "translation", {
			"test.items_one": "{{count}} élément",
			"test.items_other": "{{count}} éléments",
		});
		setLanguageChoice("fr");
		expect(t("test.fallback", { name: "MaxHQ" })).toBe("Hello MaxHQ");
		expect(t("test.items", { count: 0 })).toBe("0 élément");
		expect(t("test.items", { count: 2 })).toBe("2 éléments");
	});
	it("formats dates and numbers using the interface locale", () => {
		setLanguageChoice("de");
		expect(formatNumber(1234.5)).toBe(new Intl.NumberFormat("de").format(1234.5));
		const options = {
			year: "numeric",
			month: "long",
			day: "numeric",
			timeZone: "UTC",
		} as const;
		expect(formatDate(new Date("2026-11-13T12:00:00Z"), options)).toBe(
			new Intl.DateTimeFormat("de", options).format(new Date("2026-11-13T12:00:00Z"))
		);
	});
});
