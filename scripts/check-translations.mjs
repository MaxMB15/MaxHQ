// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { readFile, readdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const argumentsOf = (text) =>
	[...text.matchAll(/{{\s*-?\s*([^},]+)(?:,[^}]+)?\s*}}/g)]
		.map((match) => match[1].trim())
		.sort();

export const translationErrors = (english, translated) =>
	Object.entries(english).flatMap(([key, value]) => {
		const translation = translated[key];
		if (typeof translation !== "string" || !translation.trim())
			return [`${key}: missing translation`];
		return JSON.stringify(argumentsOf(value)) === JSON.stringify(argumentsOf(translation))
			? []
			: [`${key}: interpolation differs`];
	});

export const checkTranslations = async () => {
	const root = new URL("../packages/shared/src/i18n/locales/", import.meta.url);
	const errors = [];
	for (const domain of await readdir(new URL("en/", root))) {
		if (!domain.endsWith(".json")) continue;
		const english = JSON.parse(await readFile(new URL(`en/${domain}`, root), "utf8"));
		for (const language of ["nl", "de", "fr", "es"]) {
			const translated = JSON.parse(
				await readFile(new URL(`${language}/${domain}`, root), "utf8")
			);
			errors.push(
				...translationErrors(english, translated).map(
					(error) => `${language}/${domain}: ${error}`
				)
			);
		}
	}
	return errors;
};

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	const errors = await checkTranslations();
	if (errors.length) {
		console.error(errors.join("\n"));
		process.exitCode = 1;
	} else
		console.log(
			"All interface translations contain the English keys and interpolation arguments."
		);
}
