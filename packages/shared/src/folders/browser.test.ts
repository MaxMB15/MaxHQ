// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { FolderListing, Request, Response } from "../bindings";
import { mockFolders, noFolders } from "../data/mock-folders";
import { DEVBOX, MAC } from "../data/mock-view";
import { asker } from "../data/request-source";

import { chosenPath, crumbsOf, reachFolder, startOf, tooOld } from "./browser";
import { folderCalls } from "./requests";

const FOLDERS = { files: false, hidden: false };

const listing = (over: Partial<FolderListing> = {}): FolderListing => ({
	path: "/home/dev/code",
	home: "/home/dev",
	exists: true,
	entries: [
		{ name: "shop", folder: true, checkout: { project: "proj-shop", branch: "main" } },
		{ name: "run.sh", folder: false, checkout: null },
	],
	truncated: false,
	...over,
});

describe("folder calls", () => {
	it("asks the device for a listing or a new folder", async () => {
		const sent: Request[] = [];
		const calls = folderCalls(
			asker(async (request) => {
				sent.push(request);
				const value =
					request.kind === "list_folder" ? { status: "too_old" } : { status: "done" };
				return { status: "ok", data: { kind: request.kind, value } as Response };
			})
		);
		expect(await calls.listFolder(DEVBOX, "~/code", { files: true, hidden: false })).toEqual({
			status: "ok",
			data: { status: "too_old" },
		});
		await calls.listFolder(DEVBOX, "~/code", { files: false, hidden: true, filter: "sh" });
		await calls.makeFolder(DEVBOX, "/home/dev/code/new");
		expect(sent).toEqual([
			{
				kind: "list_folder",
				device: DEVBOX,
				path: "~/code",
				files: true,
				hidden: false,
				filter: null,
			},
			{
				kind: "list_folder",
				device: DEVBOX,
				path: "~/code",
				files: false,
				hidden: true,
				filter: "sh",
			},
			{ kind: "make_folder", device: DEVBOX, path: "/home/dev/code/new" },
		]);
	});
});

describe("crumbs and start", () => {
	it("steps from / down to the folder shown", () => {
		expect(crumbsOf("/home/dev/code")).toEqual([
			{ name: "/", path: "/" },
			{ name: "home", path: "/home" },
			{ name: "dev", path: "/home/dev" },
			{ name: "code", path: "/home/dev/code" },
		]);
		expect(crumbsOf("/")).toEqual([{ name: "/", path: "/" }]);
	});

	it("steps from a Windows drive or share down to the folder shown", () => {
		expect(crumbsOf("C:\\Users\\me")).toEqual([
			{ name: "C:", path: "C:\\" },
			{ name: "Users", path: "C:\\Users" },
			{ name: "me", path: "C:\\Users\\me" },
		]);
		expect(crumbsOf("\\\\nas\\share\\code")).toEqual([
			{ name: "\\\\nas\\share", path: "\\\\nas\\share\\" },
			{ name: "code", path: "\\\\nas\\share\\code" },
		]);
	});

	it("opens at the field's path, or home for anything else", () => {
		expect(startOf(" ~/code ")).toBe("~/code");
		expect(startOf("/srv")).toBe("/srv");
		expect(startOf("")).toBe("~");
		expect(startOf("owner/repo")).toBe("~");
	});
});

describe("reachFolder", () => {
	it("opens a folder that's there, with ~ read", async () => {
		const reached = await reachFolder(mockFolders(), DEVBOX, "~/code", FOLDERS);
		expect(reached.ok && reached.listing.path).toBe("/home/dev/code");
		expect(reached.ok && reached.listing.entries.map((e) => e.name)).toEqual([
			"DuskTools",
			"Mosaic",
			"notes",
			"scratch",
		]);
		expect(reached.ok && reached.selected).toBeNull();
	});

	it("walks up from a folder that isn't there and picks the name it came from", async () => {
		const reached = await reachFolder(
			mockFolders(),
			DEVBOX,
			"/home/dev/code/gone/deeper",
			FOLDERS
		);
		expect(reached.ok && reached.listing.path).toBe("/home/dev/code");
		expect(reached.ok && reached.selected).toBe("deeper");
	});

	it("says why for an older maxd or a request that failed", async () => {
		expect(await reachFolder(noFolders, DEVBOX, "~", FOLDERS)).toEqual({
			ok: false,
			reason: tooOld(),
		});
		const failing = {
			listFolder: async () => ({
				status: "error" as const,
				error: { kind: "failed" as const, message: "devbox isn't connected" },
			}),
		};
		expect(await reachFolder(failing, DEVBOX, "~", FOLDERS)).toEqual({
			ok: false,
			reason: "devbox isn't connected",
		});
	});
});

describe("chosenPath", () => {
	it("takes the picked folder, or the folder shown", () => {
		expect(chosenPath(listing(), "shop", "folder")).toBe("/home/dev/code/shop");
		expect(chosenPath(listing(), null, "folder")).toBe("/home/dev/code");
		expect(chosenPath(listing(), "run.sh", "folder")).toBe("/home/dev/code");
	});

	it("takes only a picked file where a file goes", () => {
		expect(chosenPath(listing(), "run.sh", "file")).toBe("/home/dev/code/run.sh");
		expect(chosenPath(listing(), "shop", "file")).toBeNull();
		expect(chosenPath(listing(), null, "file")).toBeNull();
	});

	it("takes nothing before a listing or in a folder that isn't there", () => {
		expect(chosenPath(null, null, "folder")).toBeNull();
		expect(chosenPath(listing({ exists: false, entries: [] }), null, "folder")).toBeNull();
	});
});

describe("sample folders", () => {
	it("lists files and dot names only when asked", async () => {
		const calls = mockFolders();
		const names = async (files: boolean, hidden: boolean) => {
			const result = await calls.listFolder(MAC, "~", { files, hidden });
			return result.status === "ok" && result.data.status === "listed"
				? result.data.listing.entries.map((e) => e.name)
				: [];
		};
		expect(await names(false, false)).toEqual(["Code", "Desktop", "Documents", "Downloads"]);
		expect(await names(true, true)).toEqual([
			".config",
			"Code",
			"Desktop",
			"Documents",
			"Downloads",
			"notes.md",
		]);
	});

	it("makes one folder in a folder that's there", async () => {
		const calls = mockFolders();
		expect(await calls.makeFolder(DEVBOX, "~/code/new")).toEqual({
			status: "ok",
			data: { status: "done" },
		});
		expect(await calls.makeFolder(DEVBOX, "~/code/new")).toEqual({
			status: "ok",
			data: { status: "refused", reason: "/home/dev/code/new is there already." },
		});
		expect(await calls.makeFolder(DEVBOX, "~/gone/new")).toEqual({
			status: "ok",
			data: { status: "refused", reason: "/home/dev/gone isn't there." },
		});
		const reached = await reachFolder(calls, DEVBOX, "~/code/new", FOLDERS);
		expect(reached.ok && reached.listing.exists).toBe(true);
	});
});
