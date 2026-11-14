// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { mockSource } from "@/data/mock-source";
import { DEVBOX } from "@maxhq/shared/data/mock-view";
import type { Entity, WorkspaceState } from "@maxhq/shared/bindings";
import { NOW } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import {
	egressText,
	hostChoices,
	isLive,
	madeWorkspaces,
	makesWorkspaces,
	planWorkspace,
	stateLook,
	stateText,
	type WorkspaceChoices,
	workspaceActions,
} from "@maxhq/shared/workspaces/state";

const loadView = async (scenario = "connected") =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false, scenario }).snapshot());

/** The mock view with devbox's `maxd` saying it makes no workspaces. */
const loadMakingNone = async () => {
	const snapshot = await mockSource({
		now: () => NOW,
		tick: false,
		scenario: "connected",
	}).snapshot();
	return applySnapshot({
		...snapshot,
		entities: snapshot.entities.map((e): Entity =>
			e.entity === "device_link" && e.device === DEVBOX
				? { ...e, capabilities: { transfers: true, tunnels: true, workspaces: false } }
				: e
		),
	});
};

const choose = (over: Partial<WorkspaceChoices> = {}): WorkspaceChoices => ({
	project: "proj-comet",
	device: DEVBOX,
	name: "",
	startAtBoot: true,
	...over,
});

describe("workspace states", () => {
	it("gives each state the N-2 dot", () => {
		const looks = (
			[
				{ state: "running" },
				{ state: "creating", step: "Making the container", done: 0, of: 5 },
				{ state: "starting" },
				{ state: "stopping" },
				{ state: "deleting" },
				{ state: "stopped" },
				{ state: "storage_locked" },
				{ state: "failed", reason: "clone refused" },
			] satisfies WorkspaceState[]
		).map((s) => [s.state, stateLook(s)]);
		expect(looks).toEqual([
			["running", "live"],
			["creating", "connecting"],
			["starting", "connecting"],
			["stopping", "connecting"],
			["deleting", "connecting"],
			["stopped", "agent_down"],
			["storage_locked", "agent_down"],
			["failed", "unreachable"],
		]);
	});

	it("says which step a new workspace is on", () => {
		expect(
			stateText({ state: "creating", step: "Checking what it can reach", done: 1, of: 5 })
		).toBe("Creating, step 2 of 5: Checking what it can reach");
		expect(stateText({ state: "storage_locked" })).toBe(
			"Storage locked. Unlock the device's storage, then start it"
		);
		expect(stateText({ state: "failed", reason: "clone refused" })).toBe(
			"Failed: Clone refused"
		);
	});

	it("names what answered when the egress check fails", () => {
		const at = new Date(2026, 9, 25, 14, 30).getTime();
		expect(
			egressText({ state: "failed", at, reached: ["192.168.1.1", "devbox.local"] }, NOW)
		).toBe(
			"Egress check failed at 14:30. 192.168.1.1 and devbox.local answered, so agents won't start until a check passes"
		);
		expect(egressText({ state: "passed", at }, NOW)).toBe(
			"Can't reach your LAN or tailnet. The public internet stays open. Checked at 14:30"
		);
	});

	it("offers only what each state allows", () => {
		expect(workspaceActions({ state: "running" })).toEqual(["stop", "delete"]);
		expect(workspaceActions({ state: "stopped" })).toEqual(["start", "delete"]);
		expect(workspaceActions({ state: "storage_locked" })).toEqual(["delete"]);
		expect(workspaceActions({ state: "starting" })).toEqual([]);
	});

	it("lists the workspaces MaxHQ made and leaves the others out", async () => {
		const view = await loadView();
		expect(madeWorkspaces(view, { device: DEVBOX }).map((w) => w.id)).toEqual(["ws-mhq-maxhq"]);
		expect(madeWorkspaces(view, { device: "dev-mac" })).toEqual([]);
	});
});

describe("new workspace spec", () => {
	it("builds the spec from the choices", async () => {
		const view = await loadView();
		const live = () => true;
		expect(planWorkspace(view, choose(), live)).toEqual({
			ok: true,
			instance: "mhq-comet",
			spec: { project: "proj-comet", device: DEVBOX, name: null, start_at_boot: true },
		});
		expect(planWorkspace(view, choose({ name: "com-2", startAtBoot: false }), live)).toEqual({
			ok: true,
			instance: "mhq-com-2",
			spec: { project: "proj-comet", device: DEVBOX, name: "com-2", start_at_boot: false },
		});
	});

	it("says why it can't make one yet", async () => {
		const view = await loadView();
		const live = () => true;
		const reason = (over: Partial<WorkspaceChoices>, isUp = live) => {
			const plan = planWorkspace(view, choose(over), isUp);
			return plan.ok ? null : plan.reason;
		};
		expect(reason({ project: null })).toBe("Pick a project.");
		expect(reason({ device: "dev-mac" })).toBe("Mac has no Incus.");
		expect(reason({}, () => false)).toBe("Connect to devbox first.");
		expect(reason({ name: "Bad Name" })).toBe(
			"Use lower-case letters, digits and hyphens for the name, up to 59 of them."
		);
		expect(reason({ project: "proj-maxhq" })).toBe("mhq-maxhq already exists on devbox.");
	});

	it("won't make one on a device whose maxd makes none", async () => {
		const view = await loadMakingNone();
		const live = () => true;
		expect(makesWorkspaces(await loadView(), DEVBOX)).toBe(true);
		expect(makesWorkspaces(view, DEVBOX)).toBe(false);
		expect(planWorkspace(view, choose(), live)).toEqual({
			ok: false,
			reason: "devbox doesn't make workspaces now.",
		});
		expect(hostChoices(view, live).find((o) => o.value === DEVBOX)).toMatchObject({
			detail: "Doesn't make workspaces now",
			disabled: true,
		});
	});

	it("needs the device's maxd to answer", async () => {
		expect(isLive(await loadView(), DEVBOX, NOW)).toBe(true);
		expect(isLive(await loadView("peer_offline"), DEVBOX, NOW)).toBe(false);
	});
});
