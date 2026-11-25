// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it } from "vitest";
import {
	configureLanguage,
	formatDate,
	formatNumber,
	i18n,
	languageFor,
	regionalLocale,
	setInterfaceOs,
	setLanguageChoice,
	t,
} from "./i18n";

afterEach(() => {
	setInterfaceOs(null);
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
	it("takes the system's first language when MaxHQ has it, and English otherwise", () => {
		expect(languageFor("system", ["pt-BR"])).toBe("en");
		expect(languageFor("system", ["pt-BR", "nl-NL"])).toBe("en");
		expect(languageFor("system", ["nl_NL"])).toBe("nl");
		expect(languageFor("system", ["NL"])).toBe("nl");
		expect(languageFor("system", ["tlh"])).toBe("en");
		expect(languageFor("system", ["not a tag"])).toBe("en");
		expect(languageFor("system", [])).toBe("en");
	});
	it("falls back to English when the platform can't say its languages", () => {
		configureLanguage({
			read: () => "system",
			write: () => {},
			systemLanguages: () => {
				throw Error("no locale");
			},
		});
		expect(i18n.language).toBe("en");
		configureLanguage({
			read: () => "system",
			write: () => {},
			systemLanguages: () => ["pt-BR"],
		});
		expect(i18n.language).toBe("en");
		configureLanguage({
			read: () => "system",
			write: () => {},
			systemLanguages: () => ["de-CH"],
		});
		expect(i18n.language).toBe("de");
	});
	it("gives a native picker MaxHQ's language in the platform's region", () => {
		expect(regionalLocale("nl", "US")).toBe("nl-US");
		expect(regionalLocale("de", "NL")).toBe("de-NL");
		expect(regionalLocale("fr", "ca")).toBe("fr-CA");
		expect(regionalLocale("es", "419")).toBe("es-419");
		expect(regionalLocale("en", null)).toBe("en");
		expect(regionalLocale("en", undefined)).toBe("en");
		expect(regionalLocale("nl", "")).toBe("nl");
		expect(regionalLocale("nl", "en-US")).toBe("nl");
		expect(regionalLocale("nl", "USA")).toBe("nl");
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

describe("wording for the system the app runs on", () => {
	it("keeps the text as written on macOS and until the app says its system", () => {
		expect(t("Keep this Mac awake")).toBe("Keep this Mac awake");
		setInterfaceOs("macos");
		expect(t("Keep this Mac awake")).toBe("Keep this Mac awake");
		expect(t("Show {{name}} in Finder", { name: "log.txt" })).toBe("Show log.txt in Finder");
	});
	it("says PC on Windows and computer on Linux", () => {
		setInterfaceOs("windows");
		expect(t("Keep this Mac awake")).toBe("Keep this PC awake");
		expect(t("Show {{name}} in Finder", { name: "log.txt" })).toBe(
			"Show log.txt in File Explorer"
		);
		setInterfaceOs("linux");
		expect(t("Keep this Mac awake")).toBe("Keep this computer awake");
		expect(t("Show {{name}} in Finder", { name: "log.txt" })).toBe(
			"Show log.txt in its folder"
		);
	});
	it("leaves text without a variant as it is", () => {
		setInterfaceOs("windows");
		expect(t("Close")).toBe(t("Close", { context: undefined }));
	});
	it("lets a call name a device's own system instead", () => {
		setInterfaceOs("windows");
		expect(t("Keep this Mac awake", { context: undefined })).toBe("Keep this Mac awake");
		expect(t("Keep this Mac awake", { context: "linux" })).toBe("Keep this computer awake");
	});
	it("finds the variant in the language shown", () => {
		configureLanguage({ read: () => "de", write: () => {}, systemLanguages: () => ["de"] });
		setInterfaceOs("windows");
		expect(t("Keep this Mac awake")).toBe("Diesen PC wach halten");
		setInterfaceOs("macos");
		expect(t("Keep this Mac awake")).not.toContain("PC");
	});
	it("picks a plural's variant by count", () => {
		i18n.addResources("en", "translation", {
			"{{count}} Macs": "{{count}} Macs",
			"{{count}} Macs_one": "{{count}} Mac",
			"{{count}} Macs_windows": "{{count}} PCs",
			"{{count}} Macs_windows_one": "{{count}} PC",
		});
		setInterfaceOs("windows");
		expect(t("{{count}} Macs", { count: 1 })).toBe("1 PC");
		expect(t("{{count}} Macs", { count: 3 })).toBe("3 PCs");
		setInterfaceOs("macos");
		expect(t("{{count}} Macs", { count: 1 })).toBe("1 Mac");
	});
});
