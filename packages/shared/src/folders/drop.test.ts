// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { Entity, FolderEntry } from "../bindings";
import { mockFolders } from "../data/mock-folders";
import { MAC } from "../data/mock-view";
import { entities, project, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";

import { checkoutProject, dropHint, dropOutcome, sameRepoFolder } from "./drop";
import type { FolderCalls } from "./requests";

const HOME = "/Users/me";

describe("dropOutcome", () => {
	it("takes a folder from this Mac on a field for this Mac", () => {
		expect(
			dropOutcome({ path: `${HOME}/code/shop`, folder: true }, "folder", true, HOME)
		).toEqual({ kind: "use", path: `${HOME}/code/shop`, note: null });
	});

	it("takes the folder a file is in where a folder goes, and says so", () => {
		expect(
			dropOutcome(
				{ path: `${HOME}/code/shop/README.md`, folder: false },
				"folder",
				true,
				HOME
			)
		).toEqual({
			kind: "use",
			path: `${HOME}/code/shop`,
			note: "Uses the folder it's in, ~/code/shop",
		});
	});

	it("takes a file where a file goes, and refuses a folder there", () => {
		expect(
			dropOutcome({ path: "/usr/local/bin/agent", folder: false }, "file", true, HOME)
		).toEqual({ kind: "use", path: "/usr/local/bin/agent", note: null });
		expect(dropOutcome({ path: "/usr/local/bin", folder: true }, "file", true, HOME)).toEqual({
			kind: "refuse",
			why: "Drop a file here, not a folder.",
		});
	});

	it("says a drop for another device is on this Mac, and never copies it", () => {
		expect(
			dropOutcome({ path: `${HOME}/code/shop`, folder: true }, "folder", false, HOME)
		).toEqual({
			kind: "elsewhere",
			folder: `${HOME}/code/shop`,
			why: "That folder is on this Mac.",
		});
		expect(
			dropOutcome({ path: `${HOME}/code/shop/a.ts`, folder: false }, "folder", false, HOME)
		).toEqual({
			kind: "elsewhere",
			folder: `${HOME}/code/shop`,
			why: "That file is on this Mac.",
		});
	});

	it("says what a drop will do while it's over the field", () => {
		expect(dropHint("folder", true)).toBe("Drop to use this folder");
		expect(dropHint("file", true)).toBe("Drop to use this file");
		expect(dropHint("folder", false)).toBe("That folder is on this Mac.");
	});
});

describe("checkoutProject", () => {
	it("learns a dropped checkout's project from a listing of its folder", async () => {
		const calls = mockFolders();
		expect(await checkoutProject(calls, MAC, `${HOME}/Code/Github/Multi/Comet`)).toBe(
			"proj-comet"
		);
		expect(await checkoutProject(calls, MAC, `${HOME}/Desktop`)).toBeNull();
		expect(await checkoutProject(calls, MAC, "/")).toBeNull();
	});

	it("asks for the dropped folder by name, so one past where a big listing ends still comes", async () => {
		const shop: FolderEntry = {
			name: "shop",
			folder: true,
			checkout: { project: "proj-shop", branch: "main" },
		};
		// A folder too big to list whole: only a filter reaches "shop".
		const listFolder = vi.fn<FolderCalls["listFolder"]>(async (_device, path, options) => ({
			status: "ok",
			data: {
				status: "listed",
				listing: {
					path,
					home: HOME,
					exists: true,
					entries: options.filter === "shop" ? [shop] : [],
					truncated: options.filter !== "shop",
				},
			},
		}));
		expect(await checkoutProject({ listFolder }, MAC, `${HOME}/code/shop`)).toBe("proj-shop");
		expect(listFolder).toHaveBeenCalledWith(MAC, `${HOME}/code`, {
			files: false,
			hidden: true,
			filter: "shop",
		});
	});

	it("knows no project when the listing fails, and never throws", async () => {
		const listFolder = vi.fn<FolderCalls["listFolder"]>(() =>
			Promise.reject(new Error("gone"))
		);
		expect(await checkoutProject({ listFolder }, MAC, `${HOME}/code/shop`)).toBeNull();
	});
});

describe("sameRepoFolder", () => {
	const dispatcher: Entity = {
		entity: "dispatcher",
		device: "dev-box",
		workspace: "ws-home",
		agent: null,
		plan: null,
	};
	const view = applySnapshot({
		seq: 1,
		entities: [
			...entities({
				projects: [
					{ ...project("proj-shop", "shop"), repo: "https://github.com/owner/shop.git" },
					{ ...project("proj-shop-box", "shop"), repo: "git@github.com:owner/shop.git" },
					project("proj-notes", "notes"),
				],
				workspaces: [
					workspace({
						id: "ws-shop",
						device: "dev-box",
						project: "proj-shop-box",
						location: { type: "folder", path: "/home/dev/code/shop" },
					}),
					workspace({
						id: "ws-box",
						device: "dev-box",
						project: "proj-notes",
						location: {
							type: "container",
							instance: "box-notes",
							path: "/home/agent/notes",
						},
					}),
					workspace({ id: "ws-home", device: "dev-box", project: "proj-notes" }),
				],
			}),
			dispatcher,
		],
	});

	it("finds the device's checkout of the same repository", () => {
		expect(sameRepoFolder(view, "proj-shop", "dev-box")).toBe("/home/dev/code/shop");
		expect(sameRepoFolder(view, "proj-shop", "dev-mac")).toBeNull();
	});

	it("leaves out containers and the dispatcher's own folder", () => {
		expect(sameRepoFolder(view, "proj-notes", "dev-box")).toBeNull();
	});
});
