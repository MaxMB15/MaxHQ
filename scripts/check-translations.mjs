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

const VARIANT = /^(.*)_(windows|linux|macos|android|ios)(_(?:zero|one|two|few|many|other))?$/s;
const OTHER_OS = { windows: "linux", linux: "windows" };
const BASE_OS = { macos: "macOS", ios: "iOS" };

/**
 * What's wrong with the system variants among a language's keys. A key
 * that names Windows or Linux (`key_windows`, `key_linux`, and with a
 * plural `key_windows_one`) needs the macOS text as its base key, its
 * sibling for the other system, and the base's arguments. A key that names
 * Android (`key_android`) needs the iOS text as its base key and its
 * arguments, and has no sibling. macOS and iOS have no variant: they read
 * the base key.
 */
export const variantErrors = (keys) =>
	Object.entries(keys).flatMap(([key, value]) => {
		const match = VARIANT.exec(key);
		if (!match) return [];
		const [, stem, os, plural = ""] = match;
		if (os in BASE_OS)
			return [`${key}: ${BASE_OS[os]} reads the base key, so drop this variant`];
		const base = stem + plural;
		if (!(base in keys)) return [`${key}: no base key ${JSON.stringify(base)}`];
		const errors = [];
		const sibling = OTHER_OS[os] && `${stem}_${OTHER_OS[os]}${plural}`;
		if (sibling && !(sibling in keys))
			errors.push(`${key}: missing ${JSON.stringify(sibling)}`);
		if (!plural && `${stem}_one` in keys && !(`${key}_one` in keys))
			errors.push(`${key}: missing ${JSON.stringify(`${key}_one`)}`);
		if (JSON.stringify(argumentsOf(value)) !== JSON.stringify(argumentsOf(keys[base])))
			errors.push(`${key}: interpolation differs from the base key`);
		return errors;
	});

export const checkTranslations = async () => {
	const root = new URL("../packages/shared/src/i18n/locales/", import.meta.url);
	const errors = [];
	for (const domain of await readdir(new URL("en/", root))) {
		if (!domain.endsWith(".json")) continue;
		const english = JSON.parse(await readFile(new URL(`en/${domain}`, root), "utf8"));
		errors.push(...variantErrors(english).map((error) => `en/${domain}: ${error}`));
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
			"All interface translations contain the English keys, their system variants and interpolation arguments."
		);
}
