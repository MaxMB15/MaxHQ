// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { WorkspaceId } from "../bindings";
import type { CopiedProject, DataSource } from "../data/source";
import { entities, workspace } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import type { PlannedCopy } from "./copy-form";
import { dismissMissed, keepMissed, missedOf, missedTitle, noteMissed } from "./missed";

const W1 = "w-1" as WorkspaceId;
const W2 = "w-2" as WorkspaceId;

const ENV = { path: ".env", reason: "no space left" };
const KEY = { path: "certs/dev.key", reason: "permission denied" };

const sourceOf = () => ({}) as DataSource;

// A folder copy to /home/dev/code/comet on the box.
const PLAN = {
	ok: true,
	project: "proj-comet",
	target: { type: "folder", device: "dev-box" },
	where: "/home/dev/code/comet",
} as unknown as PlannedCopy;

const viewWith = (landed: boolean): ViewState =>
	applySnapshot({
		seq: 1,
		entities: entities({
			workspaces: [
				workspace({ id: "w-mac", project: "proj-comet" }),
				...(landed
					? [
							workspace({
								id: "w-new",
								project: "proj-comet",
								device: "dev-box",
								location: { type: "folder", path: "/home/dev/code/comet" },
							}),
						]
					: []),
			],
		}),
	});

const placed = (failed: CopiedProject["failed"]) => ({ written: 3, failed }) as CopiedProject;

describe("noteMissed", () => {
	it("keeps each workspace's files until they're dismissed", () => {
		const source = sourceOf();
		expect(missedOf(source, W1)).toEqual([]);
		noteMissed(source, W1, [ENV]);
		noteMissed(source, W2, [KEY]);
		expect(missedOf(source, W1)).toEqual([ENV]);
		expect(missedOf(source, W2)).toEqual([KEY]);
		dismissMissed(source, W1);
		expect(missedOf(source, W1)).toEqual([]);
		expect(missedOf(source, W2)).toEqual([KEY]);
	});

	it("keeps them apart for each source", () => {
		const one = sourceOf();
		const two = sourceOf();
		noteMissed(one, W1, [ENV]);
		expect(missedOf(two, W1)).toEqual([]);
	});

	it("keeps nothing for a copy that placed every file", () => {
		const source = sourceOf();
		noteMissed(source, W1, [ENV]);
		noteMissed(source, W1, []);
		expect(missedOf(source, W1)).toEqual([ENV]);
	});

	it("puts a later copy's files in place of an earlier one's", () => {
		const source = sourceOf();
		noteMissed(source, W1, [ENV]);
		noteMissed(source, W1, [KEY]);
		expect(missedOf(source, W1)).toEqual([KEY]);
	});
});

describe("keepMissed", () => {
	it("keeps what a copy couldn't place in the workspace it made", () => {
		const source = sourceOf();
		keepMissed(source, viewWith(true), PLAN, new Set(["w-mac"]), placed([ENV, KEY]));
		expect(missedOf(source, "w-new" as WorkspaceId)).toEqual([ENV, KEY]);
	});

	it("keeps nothing for a failed copy, one that placed everything, or one not landed yet", () => {
		const source = sourceOf();
		keepMissed(source, viewWith(true), PLAN, new Set(["w-mac"]), null);
		keepMissed(source, viewWith(true), PLAN, new Set(["w-mac"]), placed([]));
		keepMissed(source, viewWith(false), PLAN, new Set(["w-mac"]), placed([ENV]));
		expect(missedOf(source, "w-new" as WorkspaceId)).toEqual([]);
	});

	it("never takes a workspace that was there before the copy", () => {
		const source = sourceOf();
		keepMissed(source, viewWith(true), PLAN, new Set(["w-mac", "w-new"]), placed([ENV]));
		expect(missedOf(source, "w-new" as WorkspaceId)).toEqual([]);
	});
});

describe("missedTitle", () => {
	it("counts the files", () => {
		expect(missedTitle(1)).toBe("1 file wasn't copied");
		expect(missedTitle(3)).toBe("3 files weren't copied");
	});
});
