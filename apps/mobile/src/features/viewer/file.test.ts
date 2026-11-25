// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Lines } from "@maxhq/shared/agent-view/highlight";
import { FULL_LIMIT } from "@maxhq/shared/agent-view/media";
import type { AgentId } from "@maxhq/shared/bindings";

import {
	binaryNote,
	columnsOf,
	fileNote,
	fileRoute,
	headerSaveText,
	LONGEST,
	lineBlocks,
	lookOf,
	mimeOf,
	noteInPlace,
	noViewNote,
	saveText,
	scrollTo,
	shownLine,
	shownTokens,
	widestLine,
} from "./file";

const AGENT = "agent-1" as AgentId;

describe("lookOf", () => {
	it.each([
		["docs/plan.md", "ios", "markdown"],
		["src/main.rs", "ios", "text"],
		["data/rows.csv", "android", "text"],
		["notes.txt", "ios", "text"],
		["shots/home.png", "ios", "image"],
		["shots/home.JPG", "android", "image"],
		["art/logo.svg", "ios", "text"],
		["specs/report.pdf", "ios", "pdf"],
		["specs/report.pdf", "android", "none"],
		["models/part.glb", "ios", "none"],
		["decks/plan.pptx", "ios", "none"],
		["bin/tool", "ios", "none"],
	])("draws %s on %s as %s", (path, os, look) => {
		expect(lookOf(path, os)).toBe(look);
	});
});

describe("fileRoute", () => {
	it("goes to the file, at the line when there's one", () => {
		expect(fileRoute(AGENT, "src/main.rs", 12)).toEqual({
			agent: AGENT,
			path: "src/main.rs",
			line: 12,
		});
		expect(fileRoute(AGENT, "src/main.rs")).toEqual({ agent: AGENT, path: "src/main.rs" });
		expect(fileRoute(AGENT, "src/main.rs", null)).toEqual({
			agent: AGENT,
			path: "src/main.rs",
		});
		expect(fileRoute(AGENT, "src/main.rs", 0)).toEqual({ agent: AGENT, path: "src/main.rs" });
	});

	it("goes nowhere for a file the viewer doesn't open", () => {
		expect(fileRoute(AGENT, "Makefile", 3)).toBeNull();
		expect(fileRoute(AGENT, "config/.env")).toBeNull();
	});
});

describe("fileNote", () => {
	it("says it's reading, with how far once the size is known", () => {
		expect(fileNote("src/main.rs", null, null)).toEqual({
			kind: "reading",
			text: "Reading main.rs…",
		});
		expect(fileNote("src/main.rs", { state: "loading", sent: 0, total: 0 }, null)?.text).toBe(
			"Reading main.rs…"
		);
		expect(
			fileNote("src/main.rs", { state: "loading", sent: 2000, total: 5000 }, null)?.text
		).toBe("Reading main.rs, 2 KB of 5 KB…");
	});

	it("offers the whole of a large file the app can take, and not of one past that", () => {
		expect(fileNote("logs/run.log", { state: "large", size: 5_000_000 }, null)).toEqual({
			kind: "large",
			text: "run.log is 5.0 MB, more than the phone opens on its own.",
			more: true,
		});
		const huge = fileNote("logs/run.log", { state: "large", size: FULL_LIMIT + 1 }, null);
		expect(huge).toMatchObject({ kind: "large", more: false });
		expect(huge?.text).toMatch(/^run\.log is .+, too large to show here\.$/);
	});

	it("says why it couldn't read one", () => {
		expect(
			fileNote("src/main.rs", { state: "failed", reason: "It's gone." }, "Update maxd.")
		).toEqual({ kind: "failed", text: "Couldn't read main.rs. It's gone." });
	});

	it("says one outside the agent's folders shows on the Mac", () => {
		const outside = { state: "failed", reason: "Outside.", refused: "outside" } as const;
		const said =
			"main.rs is outside the agent's folders. The phone can't show it. Use Show anyway on the Mac.";
		expect(fileNote("/etc/main.rs", outside, null)?.text).toBe(said);
		expect(fileNote("/etc/main.rs", outside, "Update maxd.")?.text).toBe(said);
	});

	it("adds why Show anyway can't reach the device to a bad path, which an older maxd says for one outside", () => {
		const bad = { state: "failed", reason: "Bad path.", refused: "path" } as const;
		expect(fileNote("/etc/main.rs", bad, "Update maxd.")?.text).toBe(
			"Couldn't read main.rs. Bad path. Update maxd."
		);
		expect(fileNote("/etc/main.rs", bad, null)?.text).toBe("Couldn't read main.rs. Bad path.");
	});

	it("says nothing once the file is here", () => {
		expect(
			fileNote("src/main.rs", { state: "ready", url: "file:///c/x.rs", size: 4 }, null)
		).toBeNull();
	});
});

describe("the notes for a file not drawn", () => {
	it("names a binary file", () => {
		expect(binaryNote("build/out.txt")).toBe(
			"out.txt looks like a binary file, so it isn't shown as text."
		);
	});

	it("says Android can't show a PDF, and gives the size of anything else", () => {
		expect(noViewNote("specs/report.pdf", 2000, "android")).toBe(
			"Android can't show PDFs here. Save it to open it in another app."
		);
		expect(noViewNote("models/part.glb", 2000, "ios")).toBe(
			"Can't show this file here. It's 2 KB."
		);
	});
});

describe("saveText", () => {
	const going = { state: "downloading", sent: 0, total: 0, held: null } as const;

	it("says how far a download has got, or that it waits", () => {
		expect(saveText(null)).toBeNull();
		expect(saveText({ state: "done", path: "/downloads/a.zip", skippedLinks: [] })).toBeNull();
		expect(saveText(going)).toBe("Downloading");
		expect(saveText({ ...going, sent: 1, total: 4 })).toBe("Downloading, 25%");
		expect(saveText({ ...going, held: "paused" })).toBe("Paused, reconnecting");
		expect(saveText({ ...going, held: "waiting" })).toBe("Waiting for room on this phone");
	});
});

describe("where Save's progress shows", () => {
	const going = { state: "downloading", sent: 3, total: 4, held: null } as const;
	const ready = { state: "ready", url: "file:///cache/a", size: 10 } as const;

	it("puts a note in the file's place until the phone draws it", () => {
		expect(noteInPlace("src/app.ts", null, "ios")).toBe(true);
		expect(noteInPlace("src/app.ts", { state: "loading", sent: 0, total: 0 }, "ios")).toBe(
			true
		);
		expect(noteInPlace("big.log", { state: "large", size: 9e9 }, "ios")).toBe(true);
		expect(noteInPlace("models/part.glb", ready, "ios")).toBe(true);
		expect(noteInPlace("specs/report.pdf", ready, "android")).toBe(true);
		expect(noteInPlace("specs/report.pdf", ready, "ios")).toBe(false);
		expect(noteInPlace("src/app.ts", ready, "android")).toBe(false);
	});

	it("says it under the name only while the file itself shows", () => {
		expect(headerSaveText("src/app.ts", ready, going, "ios")).toBe("Downloading, 75%");
		expect(headerSaveText("src/app.ts", ready, { ...going, held: "paused" }, "ios")).toBe(
			"Paused, reconnecting"
		);
		expect(headerSaveText("src/app.ts", ready, null, "ios")).toBeNull();
		expect(headerSaveText("big.log", { state: "large", size: 9e9 }, going, "ios")).toBeNull();
		expect(headerSaveText("models/part.glb", ready, going, "ios")).toBeNull();
	});
});

describe("lineBlocks", () => {
	const token = (content: string) => ({ content, color: "#fff" });

	it("splits the lines into numbered blocks with their colours", () => {
		const lines = ["a", "b", "c", "d", "e"];
		const colours: Lines = lines.map((line) => [token(line)]);
		const blocks = lineBlocks(lines, colours, 2);
		expect(blocks.map((block) => block.first)).toEqual([1, 3, 5]);
		expect(blocks.map((block) => block.lines)).toEqual([["a", "b"], ["c", "d"], ["e"]]);
		expect(blocks[1]?.colours).toEqual([[token("c")], [token("d")]]);
	});

	it("gives blocks without colours until they're ready, and none for no lines", () => {
		expect(lineBlocks(["a"], null)).toEqual([{ first: 1, lines: ["a"], colours: null }]);
		expect(lineBlocks([], null)).toEqual([]);
	});
});

describe("a line as the list draws it", () => {
	it("draws a tab as four spaces", () => {
		expect(shownLine("\tif (x) {\t// y")).toBe("    if (x) {    // y");
		expect(shownTokens([{ content: "\tif" }, { content: " (x)", color: "#fff" }])).toEqual([
			{ content: "    if" },
			{ content: " (x)", color: "#fff" },
		]);
	});

	it("cuts a very long line, and its colours at the same place", () => {
		const long = "a".repeat(LONGEST + 10);
		expect(shownLine(long)).toBe(`${"a".repeat(LONGEST)}…`);
		expect(shownLine("a".repeat(LONGEST))).toBe("a".repeat(LONGEST));
		const tokens = [
			{ content: "a".repeat(LONGEST - 2), color: "#111" },
			{ content: "bbbb", color: "#222" },
			{ content: "c", color: "#333" },
		];
		const shown = shownTokens(tokens);
		expect(shown).toEqual([tokens[0], { content: "bb…", color: "#222" }]);
		expect(shown.map((token) => token.content).join("")).toBe(
			shownLine(tokens.map((token) => token.content).join(""))
		);
	});

	it("counts a wide character as two columns", () => {
		expect(columnsOf("let x = 1;")).toBe(10);
		expect(columnsOf("名前 = 1")).toBe(8);
		expect(widestLine(["ab", "\tabc", "名前"])).toBe(7);
		expect(widestLine([])).toBe(0);
	});
});

describe("scrollTo", () => {
	const view = { height: 600, lineHeight: 20, pad: 8 };

	it("puts the line a third of the way down", () => {
		expect(scrollTo(101, 500, view)).toEqual({ line: 101, offset: 8 + 100 * 20 - 200 });
	});

	it("stays at the top for a line near it", () => {
		expect(scrollTo(3, 500, view)).toEqual({ line: 3, offset: 0 });
	});

	it("keeps the line inside the file", () => {
		expect(scrollTo(900, 50, view).line).toBe(50);
		expect(scrollTo(0, 50, view).line).toBe(1);
		expect(scrollTo(-4, 50, view).line).toBe(1);
		expect(scrollTo(7, 0, view).line).toBe(1);
	});
});

describe("mimeOf", () => {
	it.each([
		["shots/home.png", "image/png"],
		["shots/home.jpg", "image/jpeg"],
		["art/logo.svg", "image/svg+xml"],
		["docs/plan.md", "text/markdown"],
		["specs/report.pdf", "application/pdf"],
		["src/main.rs", "text/plain"],
		["data/rows.csv", "text/csv"],
		[
			"decks/plan.pptx",
			"application/vnd.openxmlformats-officedocument.presentationml.presentation",
		],
		["models/part.glb", "model/gltf-binary"],
	])("gives %s as %s", (path, type) => {
		expect(mimeOf(path)).toBe(type);
	});
});
