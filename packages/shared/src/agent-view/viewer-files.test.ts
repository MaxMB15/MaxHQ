// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
	CODE_LIMIT,
	editLine,
	extensionOf,
	localFileLink,
	readLine,
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

	it("reads an extension from a path", () => {
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

describe("the line a file opens at", () => {
	it("comes from the first hunk of a diff", () => {
		expect(editLine("@@ -1 +1 @@\n-a\n+b\n")).toBe(1);
		expect(editLine("@@ -3,4 +5,6 @@ fn main\n")).toBe(5);
		expect(editLine(null)).toBeUndefined();
		expect(editLine("no hunks here")).toBeUndefined();
	});

	it("comes from the start of a read's span", () => {
		expect(readLine("10-40")).toBe(10);
		expect(readLine("7")).toBe(7);
		expect(readLine(null)).toBeUndefined();
		expect(readLine("all")).toBeUndefined();
	});
});

describe("localFileLink", () => {
	it.each([
		["/Users/me/work/src/main.rs", { path: "/Users/me/work/src/main.rs" }],
		["src/main.rs", { path: "src/main.rs" }],
		["./README.md", { path: "./README.md" }],
		["<docs/my plan.md>", { path: "docs/my plan.md" }],
		["docs/my%20plan.md", { path: "docs/my plan.md" }],
		["file:///Users/me/a%20b/notes.txt", { path: "/Users/me/a b/notes.txt" }],
		["file://localhost/tmp/out.pdf", { path: "/tmp/out.pdf" }],
		["FILE:/tmp/shot.png", { path: "/tmp/shot.png" }],
		["file:///C:/work/deck.pptx", { path: "C:/work/deck.pptx" }],
		["C:\\work\\main.rs", { path: "C:\\work\\main.rs" }],
		["docs/plan.md#goals", { path: "docs/plan.md" }],
		["src/app.ts?raw", { path: "src/app.ts" }],
	])("opens %s", (url, link) => {
		expect(localFileLink(url)).toEqual(link);
	});

	it.each([
		["src/main.rs#L12", 12],
		["src/main.rs#L12-L20", 12],
		["src/main.rs#L12C4-L20C1", 12],
		["src/main.rs:12", 12],
		["src/main.rs:12:3", 12],
		["main.rs:7", 7],
		["file:///tmp/main.rs#L9", 9],
		["file:///tmp/main.rs:30", 30],
	])("reads the line in %s", (url, line) => {
		expect(localFileLink(url)?.line).toBe(line);
	});

	it("leaves out a line that isn't one", () => {
		expect(localFileLink("src/main.rs#L0")).toEqual({ path: "src/main.rs" });
		expect(localFileLink("src/main.rs:0")).toEqual({ path: "src/main.rs" });
	});

	it.each([
		"",
		"   ",
		"#section",
		"?q=1",
		"https://example.com/a.md",
		"http://example.com/a.png",
		"//cdn.example.com/a.png",
		"mailto:me@example.com",
		"mailto:notes.md",
		"tel:5551234",
		"javascript:alert(1)",
		"vscode://file/tmp/a.ts",
		"file://server/share/a.md",
		"\\\\server\\share\\a.md",
		"~/notes.md",
		"../outside.md",
		"src/../../secret.ts",
		"Makefile",
		"config/.env",
		"certs/server.pem",
		"part.step",
	])("leaves %j alone", (url) => {
		expect(localFileLink(url)).toBeNull();
	});

	it.each([
		["/work/docs", "plan.md", "/work/docs/plan.md"],
		["/work/docs", "./plan.md#L3", "/work/docs/plan.md"],
		["/work/docs", "../src/main.rs:12", "/work/src/main.rs"],
		["/work/docs/", "a/./b/../c.md", "/work/docs/a/c.md"],
		["docs", "plan.md", "docs/plan.md"],
		["C:\\work\\docs", "..\\notes.txt", "C:\\work\\notes.txt"],
	])("reads a link in %s to %s from that folder", (folder, url, path) => {
		expect(localFileLink(url, folder)?.path).toBe(path);
	});

	it("keeps an absolute path, and the line, with a folder", () => {
		expect(localFileLink("/etc/hosts.txt:4", "/work")).toEqual({
			path: "/etc/hosts.txt",
			line: 4,
		});
		expect(localFileLink("file:///tmp/a.md", "/work")).toEqual({ path: "/tmp/a.md" });
	});

	it("leaves a link that climbs out of the top alone", () => {
		expect(localFileLink("../../a.md", "/work")).toBeNull();
		expect(localFileLink("../../a.md", "docs")).toBeNull();
		expect(localFileLink("..\\..\\a.md", "C:\\work")).toBeNull();
		expect(localFileLink("https://example.com/a.md", "/work")).toBeNull();
	});
});
