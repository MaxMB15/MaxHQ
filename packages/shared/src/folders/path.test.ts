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
	isPathText,
	joinPath,
	MAX_SUGGESTIONS,
	parentOf,
	pickText,
	splitTyped,
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
		expect(expandHome("~other/code", "/home/dev")).toBe("~other/code");
		expect(expandHome("/srv", "/home/dev")).toBe("/srv");
	});

	it("shows home as ~ in a sentence", () => {
		expect(contractHome("/Users/me/code/shop", "/Users/me")).toBe("~/code/shop");
		expect(contractHome("/Users/me", "/Users/me")).toBe("~");
		expect(contractHome("/Users/meg/code", "/Users/me")).toBe("/Users/meg/code");
		expect(contractHome("/Users/me/code", null)).toBe("/Users/me/code");
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
