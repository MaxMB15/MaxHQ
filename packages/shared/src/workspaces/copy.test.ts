// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Capabilities, Entity, Workspace } from "../bindings";
import { device, entities, link, project, workspace } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import {
	cloneFolderName,
	type CopyChoices,
	copyLanded,
	firstChoices,
	planCopy,
	targetChoices,
} from "./copy";
import { canCopy, cloneLabel, cloneOf } from "./state";

// The bindings learn `copy` and a workspace's `clone` from the protocol.
const COPIES = { transfers: true, tunnels: true, workspaces: true, copy: true } as Capabilities;
const OLD = { transfers: true, tunnels: true, workspaces: true } as Capabilities;

const viewOf = (extra: Entity[] = []): ViewState =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
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
					project("proj-local", "Scratch"),
				],
				workspaces: [
					workspace({
						id: "w-mac",
						project: "proj-comet",
						location: { type: "folder", path: "/Users/dev/code/comet" },
					}),
					workspace({ id: "w-scratch", project: "proj-local" }),
				],
			}),
			...extra,
		],
	});

const choose = (over: Partial<CopyChoices> = {}): CopyChoices => ({
	from: "w-mac",
	device: "dev-box",
	kind: "folder",
	container: "",
	parent: "/home/dev/code",
	folder: "comet",
	...over,
});

const live = () => true;

const reason = (view: ViewState, over: Partial<CopyChoices>, isUp = live) => {
	const plan = planCopy(view, choose(over), isUp);
	return plan.ok ? null : plan.reason;
};

describe("copy targets", () => {
	it("clones into a folder on the device", () => {
		expect(planCopy(viewOf(), choose(), live)).toEqual({
			ok: true,
			project: "proj-comet",
			where: "/home/dev/code/comet",
			target: { type: "folder", device: "dev-box", parent: "/home/dev/code", name: "comet" },
		});
	});

	it("makes a container with the name you give", () => {
		const plan = planCopy(viewOf(), choose({ kind: "container", container: "com-2" }), live);
		expect(plan).toEqual({
			ok: true,
			project: "proj-comet",
			where: "mhq-com-2",
			target: { type: "container", device: "dev-box", name: "com-2" },
		});
	});

	it("says why it can't copy yet", () => {
		const view = viewOf();
		expect(reason(view, { from: null })).toBe("Pick a checkout to copy from.");
		expect(reason(view, { from: "w-scratch" })).toBe("Scratch has no remote to clone.");
		expect(reason(view, { device: null })).toBe("Pick a device.");
		expect(reason(view, {}, () => false)).toBe("Connect to devbox first.");
		expect(reason(view, { device: "dev-old" })).toBe(
			"The maxd on old can't take copies yet. Update it first."
		);
		expect(reason(view, { parent: "code" })).toBe(
			"Give the parent folder as a full path, starting with /."
		);
		expect(reason(view, { folder: "a/b" })).toBe(
			"Name the folder the clone goes in, with no slashes."
		);
		expect(reason(view, { kind: "container", device: "dev-mac" })).toBe("Mac has no Incus.");
	});

	it("won't clone over a folder that's already a workspace", () => {
		const there: Entity = {
			entity: "workspace",
			...workspace({
				id: "w-there",
				device: "dev-box",
				location: { type: "folder", path: "/home/dev/code/comet" },
			}),
		};
		expect(reason(viewOf([there]), { parent: "/home/dev/code/" })).toBe(
			"/home/dev/code/comet is already a workspace on devbox."
		);
	});

	it("greys out devices that can't take a copy", () => {
		const options = targetChoices(viewOf(), (d) => d !== "dev-mac");
		expect(options.map((o) => [o.label, o.detail, !!o.disabled])).toEqual([
			["devbox", "Container or folder", false],
			["Mac", "Not connected", true],
			["old", "Its maxd can't take copies yet", true],
		]);
	});

	it("starts from the first checkout, on a device other than its own", () => {
		expect(firstChoices(viewOf(), "proj-comet", {}, live)).toEqual({
			from: "w-mac",
			device: "dev-box",
			kind: "container",
			container: "",
			parent: "/Users/dev/code",
			folder: "comet",
		});
	});

	it("names the clone's folder the way git does", () => {
		expect(cloneFolderName("git@example.com:owner/comet.git")).toBe("comet");
		expect(cloneFolderName("https://example.com/owner/name/")).toBe("name");
	});

	it("finds the workspace a copy made", () => {
		const plan = planCopy(viewOf(), choose(), live);
		if (!plan.ok) throw new Error(plan.reason);
		const landed: Entity = {
			entity: "workspace",
			...workspace({
				id: "w-new",
				project: "proj-comet",
				device: "dev-box",
				location: { type: "folder", path: "/home/dev/code/comet" },
			}),
		};
		expect(copyLanded(viewOf(), plan, new Set(["w-mac"]))).toBeNull();
		expect(copyLanded(viewOf([landed]), plan, new Set(["w-mac"]))?.id).toBe("w-new");
	});
});

describe("clones", () => {
	const cloning = (clone: unknown): Workspace =>
		({ ...workspace({ id: "w1" }), clone }) as Workspace;

	it("reads a folder's clone", () => {
		expect(cloneOf(workspace({ id: "w1" }))).toBeNull();
		expect(cloneOf(cloning({ state: { state: "cancelled" } }))).toEqual({ state: "cancelled" });
	});

	it("says how a clone is going", () => {
		expect(
			cloneLabel({ state: "cloning", stage: "receiving", percent: 42, stalled: false })
		).toBe("Cloning: receiving objects, 42%");
		expect(
			cloneLabel({ state: "cloning", stage: "connecting", percent: null, stalled: false })
		).toBe("Cloning: reaching the remote");
		expect(
			cloneLabel({ state: "cloning", stage: "receiving", percent: 42, stalled: true })
		).toBe("Clone stalled at 42%. git has said nothing for a while");
		expect(cloneLabel({ state: "failed", reason: "repository not found" })).toBe(
			"Clone failed: Repository not found"
		);
		expect(cloneLabel({ state: "cancelled" })).toBe("Clone cancelled");
		expect(cloneLabel(null)).toBeNull();
	});

	it("copies only through a maxd that says it can", () => {
		const view = viewOf();
		expect(canCopy(view, "dev-box")).toBe(true);
		expect(canCopy(view, "dev-old")).toBe(false);
		expect(canCopy(view, "dev-gone")).toBe(false);
	});
});
