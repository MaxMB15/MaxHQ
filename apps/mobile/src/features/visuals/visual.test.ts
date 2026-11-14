// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { createRequire } from "node:module";

import { describe, expect, it } from "vitest";

import type { AgentId, AttachmentRef } from "@maxhq/shared/bindings";
import { THEME_VARIABLES } from "@maxhq/shared/visuals/frame";
import { INLINE_VISUAL_POLICY, inlineVisualDocument } from "@maxhq/shared/visuals/inline";

import { color } from "../../theme";
import { BUNDLED_LIBRARIES } from "./libraries";
import {
	blockedHosts,
	blockedText,
	closeVisual,
	errorText,
	linkGate,
	missingText,
	openVisual,
	shownVisual,
	VISUAL_VARIABLES,
	visualName,
	visualOfBlock,
	visualOfItem,
} from "./visual";

const AGENT = "agent-1" as AgentId;

const ATTACHMENT = {
	id: "a1",
	name: "usage.html",
	kind: "file",
	bytes: 120,
	mime: "text/html",
	path: "/home/user/.maxhq/attachments/usage.html",
} as unknown as AttachmentRef;

const FENCE = '```html visual title="Usage by week"\n<canvas id="c"></canvas>\n```';

describe("a visual's row", () => {
	it("opens an agent's visual from its attachment on the agent's device", () => {
		const visual = visualOfItem(AGENT, { title: " Usage by week ", attachment: ATTACHMENT });
		expect(visual).toEqual({
			kind: "device",
			title: "Usage by week",
			agent: AGENT,
			path: ATTACHMENT.path,
		});
		expect(visualName(visual.title)).toBe("Usage by week");
	});

	it("opens a fenced visual in a note or a reply from its page", () => {
		expect(visualOfBlock(`${FENCE}\n`)).toEqual({
			kind: "page",
			title: "Usage by week",
			html: '<canvas id="c"></canvas>',
		});
		expect(visualOfBlock("```html\n<p>x</p>\n```")).toBeNull();
		expect(visualOfBlock("Some text")).toBeNull();
	});

	it("names a visual without a title", () => {
		expect(visualName("  ")).toBe("Visual");
	});

	it("shows the visual over the app until it closes", () => {
		const visual = visualOfBlock(FENCE)!;
		openVisual(visual);
		expect(shownVisual.getState().visual).toBe(visual);
		closeVisual();
		expect(shownVisual.getState().visual).toBeNull();
	});
});

describe("a visual's page on the phone", () => {
	const libraries = new Map(BUNDLED_LIBRARIES.map((name) => [name, `/* ${name} */`]));
	const page = (html: string) =>
		inlineVisualDocument({ html, theme: "dark", variables: VISUAL_VARIABLES }, libraries);

	it("draws in the phone's colors, with every variable a page can use", () => {
		expect(Object.keys(VISUAL_VARIABLES).sort()).toEqual([...THEME_VARIABLES].sort());
		const { document } = page("<p>x</p>");
		expect(document).toContain(`--background:${color.background}`);
		expect(document).toContain(`--foreground:${color.foreground}`);
		expect(document).toContain("--chart-1:#4692f6");
		expect(document).toContain("color-scheme:dark");
	});

	it("can't reach the network", () => {
		const { document } = page('<script src="https://cdn.example.com/x.js"></script>');
		expect(document).toContain(
			`<meta http-equiv="Content-Security-Policy" content="${INLINE_VISUAL_POLICY}">`
		);
		expect(INLINE_VISUAL_POLICY).toContain("default-src 'none'");
	});

	it("carries chart.js and d3.js in the page", () => {
		expect(BUNDLED_LIBRARIES).toEqual(expect.arrayContaining(["chart.js", "d3.js"]));
		const { document, missing } = page(
			'<script src="maxhq:lib/chart.js"></script><script src="maxhq:lib/d3.js"></script>'
		);
		expect(missing).toEqual([]);
		expect(document).toContain("<script>/* chart.js */</script>");
		expect(document).toContain("<script>/* d3.js */</script>");
	});

	it("bundles the libraries Metro is told about, and only those", () => {
		const require = createRequire(import.meta.url);
		const { VISUAL_LIBRARIES } = require("../../../visual-libraries.js") as {
			VISUAL_LIBRARIES: Record<string, string>;
		};
		expect([...BUNDLED_LIBRARIES].sort()).toEqual(Object.keys(VISUAL_LIBRARIES).sort());
	});

	it("says which libraries it asked for that the phone doesn't carry", () => {
		const { missing } = page('<script src="maxhq:lib/three.js"></script>');
		expect(missing).toEqual(["maxhq:lib/three.js"]);
		expect(missingText(missing)).toBe(
			"Not found: maxhq:lib/three.js. A visual has only its own page and MaxHQ's libraries."
		);
		expect(missingText([])).toBeNull();
	});

	it("names each host it was kept from once", () => {
		const hosts = [
			{ url: "https://cdn.example.com/a.js", what: "script-src-elem" },
			{ url: "https://cdn.example.com/b.css", what: "style-src-elem" },
			{ url: "inline", what: "script-src" },
			{ url: "https://api.example.org/data", what: "connect-src" },
		].reduce<readonly string[]>(blockedHosts, []);
		expect(hosts).toEqual(["cdn.example.com", "api.example.org"]);
		expect(blockedText(hosts)).toBe(
			"Blocked loading from cdn.example.com and api.example.org. Visuals don't reach the network."
		);
		expect(blockedText([])).toBeNull();
	});

	it("says its first error and how many more came", () => {
		expect(errorText({ message: "x is not defined", line: 4 }, 1)).toBe(
			"x is not defined (line 4)"
		);
		expect(errorText({ message: "boom", line: null }, 3)).toBe("boom, and 2 more errors");
	});
});

describe("a link in a visual", () => {
	it("opens web and mail links, one a second at most", () => {
		const opens = linkGate();
		expect(opens("https://example.com", 10_000)).toBe(true);
		expect(opens("https://example.com/b", 10_500)).toBe(false);
		expect(opens("mailto:someone@example.com", 11_000)).toBe(true);
	});

	it("never opens other kinds of link", () => {
		const opens = linkGate();
		expect(opens("javascript:alert(1)", 0)).toBe(false);
		expect(opens("file:///etc/hosts", 5_000)).toBe(false);
		expect(opens("maxhq://pair", 10_000)).toBe(false);
		expect(opens("about:blank", 15_000)).toBe(false);
	});
});
