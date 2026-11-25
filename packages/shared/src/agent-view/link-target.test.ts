// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { headingSlug, linkTarget, pathFrom } from "./link-target";

const file = (path: string, line: number | null = null) => ({ kind: "file", path, line });
const none = { kind: "none" };

describe("where a link goes", () => {
	it("opens web and mail links in the browser", () => {
		expect(linkTarget("https://example.com/a?b=1#c")).toEqual({
			kind: "web",
			url: "https://example.com/a?b=1#c",
		});
		expect(linkTarget("HTTP://example.com")).toEqual({
			kind: "web",
			url: "HTTP://example.com",
		});
		expect(linkTarget("mailto:someone@example.com")).toEqual({
			kind: "web",
			url: "mailto:someone@example.com",
		});
		expect(linkTarget("//example.com/x")).toEqual({
			kind: "web",
			url: "https://example.com/x",
		});
	});

	it("moves to a target within the message", () => {
		expect(linkTarget("#setup-steps")).toEqual({ kind: "anchor", id: "setup-steps" });
		expect(linkTarget("#caf%C3%A9")).toEqual({ kind: "anchor", id: "café" });
		expect(linkTarget("#")).toEqual(none);
	});

	it("opens an absolute path, with the line after a colon", () => {
		expect(linkTarget("/home/me/app/src/main.rs")).toEqual(file("/home/me/app/src/main.rs"));
		expect(linkTarget("/home/me/app/src/main.rs:48")).toEqual(
			file("/home/me/app/src/main.rs", 48)
		);
		expect(linkTarget("/home/me/app/src/main.rs:48:7")).toEqual(
			file("/home/me/app/src/main.rs", 48)
		);
	});

	it("reads a path in angle brackets, spaces and all, as Codex writes it", () => {
		expect(linkTarget("</home/me/My App/view.tsx:12>")).toEqual(
			file("/home/me/My App/view.tsx", 12)
		);
		expect(linkTarget("/home/me/My%20App/view.tsx:12")).toEqual(
			file("/home/me/My App/view.tsx", 12)
		);
	});

	it("takes the line from a code host's #L mark", () => {
		expect(linkTarget("/home/me/app/a.ts#L10")).toEqual(file("/home/me/app/a.ts", 10));
		expect(linkTarget("src/a.ts#L10-L20")).toEqual(file("src/a.ts", 10));
		expect(linkTarget("src/a.ts#L10C5")).toEqual(file("src/a.ts", 10));
		expect(linkTarget("docs/guide.md#install")).toEqual(file("docs/guide.md"));
	});

	it("opens a relative path or a bare name from the agent's folder", () => {
		expect(linkTarget("crates/usage/src/windows.rs:12")).toEqual(
			file("crates/usage/src/windows.rs", 12)
		);
		expect(linkTarget("windows.rs:12")).toEqual(file("windows.rs", 12));
		expect(linkTarget("README.md")).toEqual(file("README.md"));
		expect(linkTarget("./docs/a.md?raw=1")).toEqual(file("./docs/a.md"));
	});

	it("opens a file URL on this machine", () => {
		expect(linkTarget("file:///home/me/a.ts")).toEqual(file("/home/me/a.ts"));
		expect(linkTarget("file://localhost/home/me/a.ts#L3")).toEqual(file("/home/me/a.ts", 3));
		expect(linkTarget("file:///C:/work/a.ts")).toEqual(file("C:/work/a.ts"));
		expect(linkTarget("file://server/share/a.ts")).toEqual(none);
	});

	it("opens a Windows path", () => {
		expect(linkTarget("C:\\work\\a.ts:4")).toEqual(file("C:\\work\\a.ts", 4));
	});

	it("can't open other schemes, network shares, home paths or nothing", () => {
		expect(linkTarget("javascript:alert(1)")).toEqual(none);
		expect(linkTarget("vscode://file/a.ts")).toEqual(none);
		expect(linkTarget("\\\\server\\share\\a.ts")).toEqual(none);
		expect(linkTarget("~/notes.md")).toEqual(none);
		expect(linkTarget("")).toEqual(none);
		expect(linkTarget(undefined)).toEqual(none);
	});
});

describe("a path read from a file's folder", () => {
	it("joins a relative path to it, and keeps an absolute one", () => {
		expect(pathFrom("docs/", "images/a.png")).toBe("docs/images/a.png");
		expect(pathFrom("docs/", "../src/a.ts")).toBe("src/a.ts");
		expect(pathFrom("docs/", "./b.md")).toBe("docs/b.md");
		expect(pathFrom("docs/", "../../a.md")).toBe("../a.md");
		expect(pathFrom("/home/me/", "../../../a.md")).toBe("/a.md");
		expect(pathFrom("/home/me/", "/etc/hosts")).toBe("/etc/hosts");
		expect(pathFrom("", "a.md")).toBe("a.md");
		expect(pathFrom("/home/me", "a.md")).toBe("/home/me/a.md");
	});

	it("joins with a Windows folder's own separator, and stops at its drive", () => {
		expect(pathFrom("C:\\work\\docs", "images/a.png")).toBe("C:\\work\\docs\\images\\a.png");
		expect(pathFrom("C:\\work", "..\\..\\a.md")).toBe("C:\\a.md");
		expect(pathFrom("docs", "..\\a.md")).toBe("a.md");
		expect(pathFrom("C:\\work", "D:\\a.md")).toBe("D:\\a.md");
	});
});

describe("a heading's id", () => {
	it("is its text in lower case, with dashes for spaces and no marks", () => {
		expect(headingSlug("Setup steps")).toBe("setup-steps");
		expect(headingSlug("  Why not keep the fixed rows? ")).toBe("why-not-keep-the-fixed-rows");
		expect(headingSlug("Über café_2")).toBe("über-café_2");
	});
});
