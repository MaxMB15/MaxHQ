// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
	CODE_LIMIT,
	extensionOf,
	fileName,
	VIEWER_EXTENSIONS,
	VIEWER_LIMIT,
	viewerKind,
	viewerLimit,
} from "./viewer-files";

describe("viewer files", () => {
	it.each([
		["docs/plan.md", "markdown"],
		["README.MARKDOWN", "markdown"],
		["shots/home.PNG", "image"],
		["out/report.pdf", "pdf"],
		["models/part.glb", "model"],
		["models/part.3mf", "model"],
		["data/rows.tsv", "table"],
		["C:\\work\\deck.pptx", "office"],
		["src/main.rs", "code"],
		["src/app.TS", "code"],
		["web/index.html", "code"],
		["config/site.conf", "code"],
		["notes.txt", "code"],
		["logs/run.log", "code"],
		["shots/logo.svg", "image"],
	])("opens %s as %s", (path, kind) => {
		expect(viewerKind(path)).toBe(kind);
	});

	it.each([
		"Makefile",
		".md",
		".env",
		"config/.env",
		"app.env",
		"certs/server.pem",
		"certs/server.key",
		"part.step",
		"part.stp",
		"a.dwg",
	])("leaves %s to the editor", (path) => {
		expect(viewerKind(path)).toBeNull();
	});

	it("reads a name and an extension from a path", () => {
		expect(fileName("a/b/c.tar.gz")).toBe("c.tar.gz");
		expect(extensionOf("a/b/c.tar.gz")).toBe("gz");
		expect(extensionOf("a/.env")).toBe("");
	});

	// The bytes come through the media cache, which only keeps what its
	// list names.
	it("has every extension in the media cache's list", () => {
		const rust = readFileSync(
			new URL("../../../../crates/client/src/media.rs", import.meta.url),
			"utf8"
		);
		const lists = [
			...rust.matchAll(/const (?:EXTENSIONS|CODE): \[&str; \d+\] = \[([^\]]*)\]/g),
		];
		expect(lists).toHaveLength(2);
		const cached = new Set(
			lists.flatMap((list) => [...list[1]!.matchAll(/"([^"]+)"/g)].map((m) => m[1]))
		);
		expect(VIEWER_EXTENSIONS.filter((extension) => !cached.has(extension))).toEqual([]);
		// Code goes out as plain text, and only code does.
		const code = /const CODE: \[&str; \d+\] = \[([^\]]*)\]/.exec(rust)?.[1] ?? "";
		const listed = [...code.matchAll(/"([^"]+)"/g)].map((m) => m[1]).sort();
		expect(listed).toEqual(
			VIEWER_EXTENSIONS.filter((e) => viewerKind(`f.${e}`) === "code").sort()
		);
	});

	it("fetches code up to its own, smaller limit", () => {
		expect(viewerLimit("src/main.rs")).toBe(CODE_LIMIT);
		expect(viewerLimit("models/part.glb")).toBe(VIEWER_LIMIT);
		expect(CODE_LIMIT).toBeLessThan(VIEWER_LIMIT);
	});
});
