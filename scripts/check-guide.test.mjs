// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { test } from "node:test";
import { guideErrors, pageSections } from "./check-guide.mjs";

const MAP = {
	settings: [{ id: "settings/editor", title: "Editor", terms: ["Open in", "Default editor"] }],
	shortcuts: [{ id: "shortcut/split-right", title: "Split right" }],
};

const EDITOR = `# Settings

## Pick an editor
<!-- id: settings/editor -->
<!-- covers: settings/editor shortcut/split-right -->

Open in picks the Default editor for files.
`;

test("passes when every entry has a section that mentions its terms", () => {
	assert.deepEqual(guideErrors(MAP, [["settings", EDITOR]]), []);
});

test("names a new setting that no section covers", () => {
	const map = {
		...MAP,
		settings: [...MAP.settings, { id: "settings/sounds", title: "Sounds", terms: ["Volume"] }],
	};
	assert.deepEqual(guideErrors(map, [["settings", EDITOR]]), [
		"settings/sounds (Sounds): no guide section covers it",
	]);
});

test("names each term the covering sections leave out, ignoring case and accents", () => {
	const page = EDITOR.replace("Default editor", "défault EDITOR").replace("Open in", "Opens");
	assert.deepEqual(guideErrors(MAP, [["settings", page]]), [
		'settings/editor: no section that covers it mentions "Open in"',
	]);
});

test("finds a term in any section that covers the entry", () => {
	const page = `${EDITOR.replace("Open in picks", "It picks")}
## Open files
<!-- id: settings/open -->
<!-- covers: settings/editor -->

Open in sends a file to it.
`;
	assert.deepEqual(guideErrors(MAP, [["settings", page]]), []);
});

test("rejects sections without an id, ids from another page, repeats and unknown entries", () => {
	const page = `${EDITOR}
## No id here

## Taken
<!-- id: settings/editor -->

## Elsewhere
<!-- id: shortcuts/list -->
<!-- covers: settings/gone -->
`;
	assert.deepEqual(guideErrors(MAP, [["settings", page]]), [
		'guide/settings.md: "No id here" has no <!-- id: settings/... -->',
		"guide/settings.md: the id shortcuts/list doesn't start with settings/",
		"settings/editor: two sections have this id",
		"shortcuts/list: covers settings/gone, which the map doesn't have",
	]);
});

test("keeps markers and headings inside code blocks as text", () => {
	const { sections } = pageSections(
		"settings",
		`${EDITOR}\n\`\`\`md\n## Not a section\n<!-- covers: settings/gone -->\n\`\`\`\n`
	);
	assert.equal(sections.length, 1);
	assert.deepEqual(sections[0].covers, ["settings/editor", "shortcut/split-right"]);
	assert.match(sections[0].text, /## Not a section/);
});
