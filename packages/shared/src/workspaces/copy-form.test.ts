// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { Capabilities } from "../bindings";
import { copySize } from "../copy/defaults";
import type { CopiedProject, IgnoredFile, TransferProgress } from "../data/source";
import { device, entities, link, project, workspace } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import type { CopyPlan } from "./copy";
import {
	allOrNone,
	allTicked,
	copyBlocked,
	type CopyRun,
	copyTitle,
	kindLabel,
	listedFiles,
	listsIgnored,
	loadListing,
	openingChoices,
	type PlannedCopy,
	runCopy,
	shownListing,
	tickedFor,
	tickedPaths,
	tickedText,
	transferPercent,
	transferText,
	withTick,
} from "./copy-form";

// The bindings learn `copy` from the protocol.
const COPIES = { transfers: true, tunnels: true, workspaces: true, copy: true } as Capabilities;
const OLD = { transfers: true, tunnels: true, workspaces: true } as Capabilities;

const view: ViewState = applySnapshot({
	seq: 1,
	entities: entities({
		devices: [
			device({ id: "dev-mac", name: "studio", os: "macos" }),
			device({
				id: "dev-box",
				name: "devbox",
				workspace_host: { networks: ["agentbr0"], pools: ["default"] },
			}),
			device({ id: "dev-old", name: "old" }),
		],
		links: [
			link({
				id: "mac",
				name: "Mac",
				device: "dev-mac",
				route: { type: "local" },
				capabilities: COPIES,
			}),
			link({ id: "box", name: "devbox", device: "dev-box", capabilities: COPIES }),
			link({ id: "old", name: "old", device: "dev-old", capabilities: OLD }),
		],
		projects: [
			{ ...project("proj-comet", "Comet"), repo: "git@example.com:owner/comet.git" },
			{ ...project("proj-atlas", "Atlas"), repo: "git@example.com:owner/atlas.git" },
			project("proj-local", "Scratch"),
		],
		workspaces: [
			workspace({
				id: "w-mac",
				project: "proj-comet",
				location: { type: "folder", path: "/Users/dev/code/comet" },
			}),
			workspace({
				id: "w-atlas",
				project: "proj-atlas",
				location: { type: "folder", path: "/Users/dev/code/atlas" },
			}),
			workspace({ id: "w-old", project: "proj-comet", device: "dev-old" }),
			workspace({ id: "w-scratch", project: "proj-local" }),
		],
	}),
});

const live = () => true;

const files: IgnoredFile[] = [
	{ path: ".env", size: 120 },
	{ path: "web/.env.local", size: 80 },
	{ path: "data/dump.sql", size: 5_000 },
];

const plan: PlannedCopy = {
	ok: true,
	project: "proj-comet",
	where: "/home/dev/code/comet",
	target: { type: "folder", device: "dev-box", parent: "/home/dev/code", name: "comet" },
};

const progress = (over: Partial<TransferProgress> = {}): TransferProgress => ({
	transfer: "t-1",
	sent: 512,
	total: 2048,
	file: ".env",
	state: "uploading",
	...over,
});

describe("what the copy form opens with", () => {
	it("starts from the project asked for, its first checkout and a device that takes it", () => {
		expect(openingChoices(view, { project: "proj-comet" }, live)).toEqual({
			project: "proj-comet",
			from: "w-mac",
			device: "dev-box",
			kind: "container",
			container: "",
			parent: "/Users/dev/code",
			folder: "comet",
		});
	});

	it("takes the project from the checkout asked for, and the device to copy onto", () => {
		const choices = openingChoices(view, { from: "w-old", device: "dev-mac" }, live);
		expect(choices).toMatchObject({
			project: "proj-comet",
			from: "w-old",
			device: "dev-mac",
			kind: "folder",
		});
	});

	it("falls back on the first project with a remote, by name", () => {
		expect(openingChoices(view, {}, live).project).toBe("proj-atlas");
	});

	it("keeps the device when no project can copy", () => {
		const empty = applySnapshot({ seq: 1, entities: [] });
		expect(openingChoices(empty, { device: "dev-box" }, live)).toEqual({
			project: null,
			from: null,
			device: "dev-box",
			kind: "folder",
			container: "",
			parent: "",
			folder: "",
		});
	});
});

describe("the ignored files it shows", () => {
	const listed = { state: "listed", files, truncated: false } as const;

	it("lists them only where the checkout's maxd takes copies", () => {
		expect(listsIgnored(view, "w-mac")).toBe(true);
		expect(listsIgnored(view, "w-old")).toBe(false);
		expect(listsIgnored(view, null)).toBe(false);
	});

	it("shows the listing for the checkout picked, and loading for another", () => {
		expect(shownListing(view, "w-mac", { for: "w-mac", listing: listed })).toBe(listed);
		expect(shownListing(view, "w-atlas", { for: "w-mac", listing: listed })).toEqual({
			state: "loading",
		});
		expect(shownListing(view, null, { for: null, listing: listed })).toEqual({
			state: "unlisted",
			why: "",
		});
	});

	it("says an old maxd can't list them, so only the repo goes", () => {
		expect(shownListing(view, "w-old", { for: null, listing: { state: "loading" } })).toEqual({
			state: "unlisted",
			why: "The maxd on old can't list ignored files yet, so only the repo is cloned.",
		});
	});

	it("loads a listing, and turns a failure into one", async () => {
		const found = { listIgnoredFiles: async () => ({ files, truncated: true }) };
		await expect(loadListing(found, "w-mac")).resolves.toEqual({
			state: "listed",
			files,
			truncated: true,
		});
		const broken = {
			listIgnoredFiles: async () => {
				throw new Error("devbox isn't connected");
			},
		};
		await expect(loadListing(broken, "w-mac")).resolves.toEqual({
			state: "failed",
			reason: "devbox isn't connected",
		});
		const silent = { listIgnoredFiles: () => Promise.reject(new Error("")) };
		await expect(loadListing(silent, "w-mac")).resolves.toEqual({
			state: "failed",
			reason: "Couldn't list the ignored files",
		});
		expect(listedFiles({ state: "loading" })).toEqual([]);
		expect(listedFiles(listed)).toBe(files);
	});
});

describe("the files ticked", () => {
	it("ticks the env files to start with, and keeps your ticks for that checkout", () => {
		const fresh = tickedFor("w-mac", { for: null, ticked: new Set() }, files);
		expect([...fresh]).toEqual([".env", "web/.env.local"]);
		const mine = new Set(["data/dump.sql"]);
		expect(tickedFor("w-mac", { for: "w-mac", ticked: mine }, files)).toBe(mine);
		expect([...tickedFor("w-atlas", { for: "w-mac", ticked: mine }, files)]).toEqual([
			".env",
			"web/.env.local",
		]);
	});

	it("ticks and unticks one, and all or none", () => {
		const ticked = new Set([".env"]);
		expect([...withTick(ticked, "data/dump.sql", true)]).toEqual([".env", "data/dump.sql"]);
		expect([...withTick(ticked, ".env", false)]).toEqual([]);
		expect([...ticked]).toEqual([".env"]);

		const some = copySize(files, ticked);
		expect(allTicked(files, some)).toBe(false);
		expect([...allOrNone(files, some)]).toEqual([".env", "web/.env.local", "data/dump.sql"]);
		const every = copySize(files, new Set(files.map((f) => f.path)));
		expect(allTicked(files, every)).toBe(true);
		expect([...allOrNone(files, every)]).toEqual([]);
	});

	it("sends the ticked paths in the listing's order, and counts them", () => {
		const ticked = new Set(["data/dump.sql", ".env"]);
		expect(tickedPaths(files, ticked)).toEqual([".env", "data/dump.sql"]);
		expect(tickedText(copySize(files, ticked), files.length)).toBe("2 of 3, 5 KB");
		expect(tickedText(copySize(files, new Set()), files.length)).toBe("0 of 3");
	});
});

describe("why Copy is off", () => {
	const listed = { state: "listed", files, truncated: false } as const;
	const fine = copySize(files, new Set([".env"]));

	it("gives the plan's reason, then waits for the listing, then checks the size", () => {
		const unplanned: CopyPlan = { ok: false, reason: "Pick a device." };
		expect(copyBlocked(unplanned, listed, fine)).toBe("Pick a device.");
		expect(copyBlocked(plan, { state: "loading" }, fine)).toBe("Listing the ignored files.");
		const huge = copySize([{ path: "big.bin", size: 3 * 1024 ** 3 }], new Set(["big.bin"]));
		expect(copyBlocked(plan, listed, huge)).toBe(
			"That's more than one copy carries, 2 GB or 20,000 files. Untick some."
		);
		expect(copyBlocked(plan, listed, fine)).toBeNull();
		expect(copyBlocked(plan, { state: "failed", reason: "no" }, fine)).toBeNull();
	});
});

describe("a copy on its way", () => {
	it("names where it lands", () => {
		expect(kindLabel("container")).toBe("New container");
		expect(kindLabel("folder")).toBe("Folder");
	});

	it("says how far the files got, or how many wait", () => {
		expect(transferText(progress(), 3)).toBe("Copying files: 512 B of 2 KB");
		expect(transferText(progress({ state: "paused" }), 3)).toBe(
			"Paused while the link is down: 512 B of 2 KB"
		);
		expect(transferText(null, 3)).toBe("3 files to copy once it's ready.");
		expect(transferPercent(progress())).toBe(25);
		expect(transferPercent(progress({ total: 0, sent: 0 }))).toBeNull();
	});

	it("titles the copy by how far it got", () => {
		const placed: CopiedProject = { workspace: "w-new", written: 2, failed: [] };
		const runs: CopyRun[] = [
			{ phase: "copying", transfer: null },
			{ phase: "done", placed },
			{ phase: "failed", reason: "no" },
		];
		expect(runs.map((run) => copyTitle(view, plan, run))).toEqual([
			"Copying Comet to devbox",
			"Copied Comet to devbox",
			"The copy didn't finish",
		]);
	});

	it("runs the copy, telling each step, after what goes first", async () => {
		const order: string[] = [];
		const placed: CopiedProject = { workspace: "w-new", written: 1, failed: [] };
		const copyProject = vi.fn(async (_from, _target, _files, onProgress) => {
			order.push("copy");
			onProgress(progress());
			return placed;
		});
		const shown: CopyRun[] = [];
		const result = await runCopy(
			{ copyProject },
			"w-mac",
			plan,
			[".env"],
			(run) => shown.push(run),
			async () => {
				order.push("first");
			}
		);
		expect(result).toBe(placed);
		expect(order).toEqual(["first", "copy"]);
		expect(copyProject).toHaveBeenCalledWith(
			"w-mac",
			plan.target,
			[".env"],
			expect.any(Function)
		);
		expect(shown).toEqual([
			{ phase: "copying", transfer: null },
			{ phase: "copying", transfer: progress() },
			{ phase: "done", placed },
		]);
	});

	it("says why a copy failed", async () => {
		const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
		const shown: CopyRun[] = [];
		const failing = {
			copyProject: async (): Promise<CopiedProject> => {
				throw new Error("The new workspace went away before it was ready");
			},
		};
		await expect(runCopy(failing, "w-mac", plan, [], (run) => shown.push(run))).resolves.toBe(
			null
		);
		expect(shown.at(-1)).toEqual({
			phase: "failed",
			reason: "The new workspace went away before it was ready",
		});
		const silent = { copyProject: () => Promise.reject(new Error("")) };
		await runCopy(silent, "w-mac", plan, [], (run) => shown.push(run));
		expect(shown.at(-1)).toEqual({ phase: "failed", reason: "Couldn't reach the device" });
		warn.mockRestore();
	});
});
