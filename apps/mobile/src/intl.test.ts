// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { expect, it } from "vitest";

it("starts localized formatting without the optional native Intl APIs", async () => {
	const names = [
		"Locale",
		"getCanonicalLocales",
		"PluralRules",
		"ListFormat",
		"RelativeTimeFormat",
	] as const;
	const originals = names.map(
		(name) => [name, Object.getOwnPropertyDescriptor(Intl, name)!] as const
	);
	try {
		for (const name of names) Reflect.deleteProperty(Intl, name);
		await import("./intl");
		expect(Intl.getCanonicalLocales("NL-nl")).toEqual(["nl-NL"]);
		for (const locale of ["en", "nl", "de", "fr", "es"]) {
			expect(new Intl.PluralRules(locale).select(2)).toBe("other");
			expect(new Intl.ListFormat(locale).format(["a", "b"])).toContain("a");
			expect(new Intl.RelativeTimeFormat(locale).format(-1, "day")).toBeTruthy();
		}
	} finally {
		for (const [name, descriptor] of originals) Object.defineProperty(Intl, name, descriptor);
	}
});
