// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Checks that the guide keeps up with the app. Each `##` section of a page
// in guide/ carries its id in a comment, such as <!-- id: ports/forward -->,
// and names the map entries it explains in another, such as
// <!-- covers: settings/editor shortcut/split-right -->. Every entry in
// guide/map.json needs a section that names it, and the sections that name
// an entry must between them mention each of its terms. maxd reads the
// sections the same way.

import { readFile, readdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const ID = /^<!--\s*id:\s*(\S+)\s*-->$/;
const COVERS = /^<!--\s*covers:\s*(.*?)\s*-->$/;

/** Text without case, accents or runs of whitespace, for finding a term. */
export const fold = (text) =>
	text.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase().replace(/\s+/g, " ");

/**
 * The sections of one page, `name` without its `.md`: each one's id, title,
 * the entries it covers and its text without the comments, and the problems
 * with them.
 */
export const pageSections = (name, text) => {
	const sections = [];
	const errors = [];
	let fenced = false;
	let section = null;
	for (const line of text.split("\n")) {
		if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
		const trimmed = line.trim();
		if (!fenced && line.startsWith("## ")) {
			section = { id: null, title: line.slice(3).trim(), covers: [], lines: [line] };
			sections.push(section);
			continue;
		}
		if (!section) continue;
		const id = !fenced && trimmed.match(ID);
		const covers = !fenced && trimmed.match(COVERS);
		if (id) section.id = id[1];
		else if (covers) section.covers.push(...covers[1].split(/\s+/).filter(Boolean));
		else section.lines.push(line);
	}
	for (const each of sections) {
		if (!each.id)
			errors.push(`guide/${name}.md: "${each.title}" has no <!-- id: ${name}/... -->`);
		else if (!each.id.startsWith(`${name}/`))
			errors.push(`guide/${name}.md: the id ${each.id} doesn't start with ${name}/`);
	}
	return {
		sections: sections.map(({ lines, ...rest }) => ({ ...rest, text: lines.join("\n") })),
		errors,
	};
};

/** The entries of the map, from every part. */
export const mapEntries = (map) => Object.values(map).flatMap((part) => part);

/** What's missing from the guide, one line each. */
export const guideErrors = (map, pages) => {
	const errors = [];
	const sections = [];
	for (const [name, text] of pages) {
		const page = pageSections(name, text);
		errors.push(...page.errors);
		sections.push(...page.sections);
	}
	const seen = new Set();
	for (const section of sections) {
		if (!section.id) continue;
		if (seen.has(section.id)) errors.push(`${section.id}: two sections have this id`);
		seen.add(section.id);
	}
	const entries = mapEntries(map);
	const known = new Set(entries.map((entry) => entry.id));
	for (const section of sections) {
		for (const id of section.covers) {
			if (!known.has(id))
				errors.push(`${section.id}: covers ${id}, which the map doesn't have`);
		}
	}
	for (const entry of entries) {
		const naming = sections.filter((section) => section.covers.includes(entry.id));
		if (naming.length === 0) {
			errors.push(`${entry.id} (${entry.title}): no guide section covers it`);
			continue;
		}
		const text = fold(naming.map((section) => section.text).join("\n"));
		for (const term of entry.terms ?? []) {
			if (!text.includes(fold(term)))
				errors.push(`${entry.id}: no section that covers it mentions "${term}"`);
		}
	}
	return errors;
};

/** The guide's pages, by name, and its map. */
export const readGuide = async (dir) => {
	const pages = [];
	for (const file of (await readdir(dir)).sort()) {
		if (file.endsWith(".md"))
			pages.push([file.slice(0, -3), await readFile(new URL(file, dir), "utf8")]);
	}
	return { map: JSON.parse(await readFile(new URL("map.json", dir), "utf8")), pages };
};

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	const { map, pages } = await readGuide(new URL("../guide/", import.meta.url));
	const errors = guideErrors(map, pages);
	if (errors.length) {
		console.error(`The guide is missing:\n${errors.join("\n")}`);
		process.exitCode = 1;
	} else console.log(`The guide covers all ${mapEntries(map).length} entries of its map.`);
}
