// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { FolderEntry } from "../bindings";

import {
	baseName,
	checkoutTag,
	contractHome,
	expandHome,
	fitting,
	folderNameProblem,
	isAbsolutePath,
	isPathText,
	joinPath,
	MAX_SUGGESTIONS,
	namesOf,
	parentOf,
	pathStyleOf,
	pickText,
	rootOf,
	separatorOf,
	splitTyped,
	trimSeparators,
} from "./path";

const folder = (name: string, branch?: string | null): FolderEntry => ({
	name,
	folder: true,
	checkout: branch === undefined ? null : { project: "proj-shop", branch },
});

const file = (name: string): FolderEntry => ({ name, folder: false, checkout: null });

describe("path text", () => {
	it("reads full paths and paths from home, and nothing else", () => {
		expect(["/", "/srv", "~", "~/code"].map(isPathText)).toEqual([true, true, true, true]);
		expect(["", "claude-agent", "owner/repo", "~other", "https://x"].map(isPathText)).toEqual([
			false,
			false,
			false,
			false,
			false,
		]);
	});

	it("reads ~ as home once home is known", () => {
		expect(expandHome("~", "/home/dev")).toBe("/home/dev");
		expect(expandHome("~/code", "/home/dev")).toBe("/home/dev/code");
		expect(expandHome("~/code", "/")).toBe("/code");
		expect(expandHome("~/code", null)).toBe("~/code");
		// A device that can't tell its home leaves ~ as typed, never "/code".
		expect(expandHome("~/code", "")).toBe("~/code");
		expect(expandHome("~", "")).toBe("~");
		expect(expandHome("~other/code", "/home/dev")).toBe("~other/code");
		expect(expandHome("/srv", "/home/dev")).toBe("/srv");
	});

	it("shows home as ~ in a sentence", () => {
		expect(contractHome("/Users/me/code/shop", "/Users/me")).toBe("~/code/shop");
		expect(contractHome("/Users/me", "/Users/me")).toBe("~");
		expect(contractHome("/Users/meg/code", "/Users/me")).toBe("/Users/meg/code");
		expect(contractHome("/Users/me/code", null)).toBe("/Users/me/code");
		expect(contractHome("/Users/me/code", "")).toBe("/Users/me/code");
	});

	it("names a path's folder, last name and children", () => {
		expect(parentOf("/Users/me/code/")).toBe("/Users/me");
		expect(parentOf("/srv")).toBe("/");
		expect(parentOf("/")).toBeNull();
		expect(parentOf("claude-agent")).toBeNull();
		expect(baseName("/Users/me/code/")).toBe("code");
		expect(joinPath("/", "srv")).toBe("/srv");
		expect(joinPath("/srv", "app")).toBe("/srv/app");
	});
});

describe("Windows paths", () => {
	it("tells a Windows path by its drive, share or backslashes, whatever system the app is on", () => {
		expect(
			["C:\\Users\\me", "c:/code", "D:", "\\\\nas\\share", "src\\main.rs"].map(pathStyleOf)
		).toEqual(["windows", "windows", "windows", "windows", "windows"]);
		expect(
			["/Users/me", "~/code", "owner/repo", "a\\b/c", "notes.md"].map(pathStyleOf)
		).toEqual(["posix", "posix", "posix", "posix", "posix"]);
		expect(["C:\\", "C:/x", "\\\\nas\\share", "/srv"].map(isAbsolutePath)).toEqual([
			true,
			true,
			true,
			true,
		]);
		expect(["C:x", "code\\shop", "~\\code"].map(isAbsolutePath)).toEqual([false, false, false]);
	});

	it("finds a path's root: a drive, a share or /", () => {
		expect(rootOf("C:\\Users\\me")).toBe("C:\\");
		expect(rootOf("C:/Users")).toBe("C:/");
		expect(rootOf("C:")).toBe("C:");
		expect(rootOf("\\\\nas\\share\\code")).toBe("\\\\nas\\share\\");
		expect(rootOf("\\\\nas\\share")).toBe("\\\\nas\\share");
		expect(rootOf("/srv")).toBe("/");
		expect(rootOf("code\\shop")).toBe("");
		expect(namesOf("C:\\Users\\me\\")).toEqual(["Users", "me"]);
		expect(namesOf("\\\\nas\\share\\code\\shop")).toEqual(["code", "shop"]);
		expect(namesOf("/Users/me/")).toEqual(["Users", "me"]);
	});

	it("names a Windows path's folder and last name, and joins with its separator", () => {
		expect(parentOf("C:\\Users\\me\\code\\")).toBe("C:\\Users\\me");
		expect(parentOf("C:\\Users")).toBe("C:\\");
		expect(parentOf("C:\\")).toBeNull();
		expect(parentOf("C:/Users/me")).toBe("C:/Users");
		expect(parentOf("\\\\nas\\share\\code")).toBe("\\\\nas\\share\\");
		expect(parentOf("\\\\nas\\share")).toBeNull();
		expect(parentOf("src\\main.rs")).toBe("src");
		expect(baseName("C:\\Users\\me\\code\\")).toBe("code");
		expect(baseName("C:/Users/me")).toBe("me");
		expect(baseName("\\\\nas\\share\\notes.md")).toBe("notes.md");
		expect(baseName("C:\\")).toBe("");
		expect(baseName("\\\\nas\\share")).toBe("");
		expect(joinPath("C:\\Users", "me")).toBe("C:\\Users\\me");
		expect(joinPath("C:\\", "code")).toBe("C:\\code");
		expect(joinPath("C:", "code")).toBe("C:\\code");
		expect(joinPath("C:/code", "shop")).toBe("C:/code/shop");
		expect(joinPath("\\\\nas\\share", "code")).toBe("\\\\nas\\share\\code");
		expect(separatorOf("C:/code")).toBe("/");
		expect(separatorOf("C:\\code")).toBe("\\");
		expect(separatorOf("/srv")).toBe("/");
	});

	it("trims a path's end and keeps its root whole", () => {
		expect(trimSeparators("C:\\code\\\\")).toBe("C:\\code");
		expect(trimSeparators("C:\\")).toBe("C:\\");
		expect(trimSeparators("\\\\nas\\share\\")).toBe("\\\\nas\\share\\");
		expect(trimSeparators("/srv//")).toBe("/srv");
		expect(trimSeparators("/")).toBe("/");
		// A backslash is part of a name on a Mac or Linux.
		expect(trimSeparators("/srv/odd\\")).toBe("/srv/odd\\");
	});

	it("reads ~ with a Windows home and shows the home as ~", () => {
		expect(isPathText("~\\code")).toBe(true);
		expect(isPathText("C:\\")).toBe(true);
		expect(isPathText("\\\\nas\\share")).toBe(true);
		expect(expandHome("~", "C:\\Users\\me")).toBe("C:\\Users\\me");
		expect(expandHome("~/code/shop", "C:\\Users\\me")).toBe("C:\\Users\\me\\code\\shop");
		expect(expandHome("~\\code", "C:\\Users\\me")).toBe("C:\\Users\\me\\code");
		expect(contractHome("C:\\Users\\me\\code", "C:\\Users\\me")).toBe("~\\code");
		expect(contractHome("c:\\users\\ME\\code", "C:\\Users\\me")).toBe("~\\code");
		expect(contractHome("C:\\Users\\me", "C:\\Users\\me\\")).toBe("~");
		expect(contractHome("C:\\Users\\meg", "C:\\Users\\me")).toBe("C:\\Users\\meg");
		expect(contractHome("C:\\code", "C:\\")).toBe("C:\\code");
	});

	it("splits typed Windows text and completes with its separator", () => {
		expect(splitTyped("C:\\Us")).toEqual({ folder: "C:\\", shown: "C:\\", name: "Us" });
		expect(splitTyped("C:\\Users\\me\\co")).toEqual({
			folder: "C:\\Users\\me",
			shown: "C:\\Users\\me\\",
			name: "co",
		});
		expect(splitTyped("C:")).toEqual({ folder: "C:\\", shown: "C:\\", name: "" });
		expect(splitTyped("~\\co")).toEqual({ folder: "~", shown: "~\\", name: "co" });
		expect(splitTyped("\\\\nas\\share\\co")).toEqual({
			folder: "\\\\nas\\share\\",
			shown: "\\\\nas\\share\\",
			name: "co",
		});
		const typed = { folder: "C:\\code", shown: "C:\\code\\", name: "sh" };
		expect(pickText(typed, folder("shop"), true)).toBe("C:\\code\\shop\\");
		expect(pickText(typed, file("shop.md"), true)).toBe("C:\\code\\shop.md");
	});

	it("keeps backslashes out of a new Windows folder's name only", () => {
		expect(folderNameProblem("a\\b", "windows")).toBe("A folder's name can't have a slash.");
		expect(folderNameProblem("a\\b")).toBeNull();
	});
});

describe("splitTyped", () => {
	it("splits a path into the folder to list and the name typed in it", () => {
		expect(splitTyped("~/code/sh")).toEqual({ folder: "~/code", shown: "~/code/", name: "sh" });
		expect(splitTyped("~/")).toEqual({ folder: "~", shown: "~/", name: "" });
		expect(splitTyped("~")).toEqual({ folder: "~", shown: "~/", name: "" });
		expect(splitTyped("/sr")).toEqual({ folder: "/", shown: "/", name: "sr" });
		expect(splitTyped("/srv/")).toEqual({ folder: "/srv", shown: "/srv/", name: "" });
	});

	it("gives no suggestions for a name on PATH or a repository", () => {
		expect(splitTyped("claude-agent")).toBeNull();
		expect(splitTyped("owner/repo")).toBeNull();
		expect(splitTyped("")).toBeNull();
	});
});

describe("fitting", () => {
	const entries = [
		folder(".git"),
		folder("Desktop"),
		folder("shop", "main"),
		folder("workshop"),
		file("shop.md"),
	];

	it("puts names that start with what you typed before names that hold it", () => {
		expect(fitting(entries, "sho", "folder").map((e) => e.name)).toEqual(["shop", "workshop"]);
		expect(fitting(entries, "SHO", "folder").map((e) => e.name)).toEqual(["shop", "workshop"]);
	});

	it("shows files only where a file goes", () => {
		expect(fitting(entries, "shop", "file").map((e) => e.name)).toEqual([
			"shop",
			"shop.md",
			"workshop",
		]);
	});

	it("shows dot names only once you type a dot", () => {
		expect(fitting(entries, "", "folder").map((e) => e.name)).toEqual([
			"Desktop",
			"shop",
			"workshop",
		]);
		expect(fitting(entries, ".", "folder").map((e) => e.name)).toEqual([".git"]);
	});

	it("shows as many as a field has room for", () => {
		const many = Array.from({ length: 20 }, (_, i) => folder(`app-${i}`));
		expect(fitting(many, "app", "folder")).toHaveLength(MAX_SUGGESTIONS);
	});
});

describe("picks and tags", () => {
	const typed = { folder: "~/code", shown: "~/code/", name: "sh" };

	it("adds a slash after a folder on Tab, so you can keep typing", () => {
		expect(pickText(typed, folder("shop"), true)).toBe("~/code/shop/");
		expect(pickText(typed, folder("shop"), false)).toBe("~/code/shop");
		expect(pickText(typed, file("shop.md"), true)).toBe("~/code/shop.md");
	});

	it("tags a checkout with its branch", () => {
		expect(checkoutTag(folder("shop", "main"))).toBe("git · main");
		expect(checkoutTag(folder("shop", null))).toBe("git");
		expect(checkoutTag(folder("shop"))).toBeNull();
	});

	it("says why a new folder can't take a name", () => {
		expect(folderNameProblem(" ")).toBe("Name the new folder.");
		expect(folderNameProblem("a/b")).toBe("A folder's name can't have a slash.");
		expect(folderNameProblem("..")).toBe("A folder can't be called . or ..");
		expect(folderNameProblem(" scratch ")).toBeNull();
	});
});
