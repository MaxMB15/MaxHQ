// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { ManagedWorkspace, Workspace, WorkspaceState } from "../bindings";
import { workspace } from "../test/build";
import {
	creationSteps,
	madeInstance,
	madeOrFailed,
	type SeenSteps,
	seeStep,
	stepsLeft,
} from "./creation";

const creating = (done: number, of: number, step: string): WorkspaceState => ({
	state: "creating",
	done,
	of,
	step,
});

const made = (state: WorkspaceState): Workspace =>
	workspace({
		id: "w1",
		location: { type: "container", instance: "mhq-app", path: "/home/dev/app" },
		managed: { state } as ManagedWorkspace,
	});

describe("madeInstance", () => {
	it("takes the device's name once it gives one", () => {
		expect(madeInstance(undefined, "mhq-planned")).toBe("mhq-planned");
		expect(madeInstance(made({ state: "running" }), "mhq-planned")).toBe("mhq-app");
	});
});

describe("madeOrFailed", () => {
	it("is done once it runs, fails or goes", () => {
		expect(madeOrFailed(made(creating(1, 4, "Cloning")))).toBe(false);
		expect(madeOrFailed(made({ state: "running" }))).toBe(true);
		expect(madeOrFailed(made({ state: "failed", reason: "no space" }))).toBe(true);
		expect(madeOrFailed(undefined)).toBe(true);
	});
});

describe("creation steps", () => {
	it("keeps each step the device names, by its place", () => {
		let seen: SeenSteps = new Map();
		seen = seeStep(seen, creating(0, 3, "Making the container"));
		const same = seeStep(seen, creating(0, 3, "Making the container"));
		expect(same).toBe(seen);
		seen = seeStep(seen, creating(1, 3, "Cloning"));
		expect([...seen]).toEqual([
			[0, "Making the container"],
			[1, "Cloning"],
		]);
		expect(seeStep(seen, { state: "running" })).toBe(seen);
	});

	it("marks the steps done, running or failed as the state has it", () => {
		const seen: SeenSteps = new Map([
			[1, "Cloning"],
			[0, "Making the container"],
		]);
		const looks = (state: WorkspaceState | undefined) =>
			creationSteps(seen, state).map((s) => [s.at, s.text, s.look, s.label]);
		expect(looks(creating(1, 3, "Cloning"))).toEqual([
			[0, "Making the container", "live", "Done"],
			[1, "Cloning", "connecting", "Running"],
		]);
		expect(looks({ state: "failed", reason: "no space" })).toEqual([
			[0, "Making the container", "live", "Done"],
			[1, "Cloning", "unreachable", "Failed"],
		]);
		expect(looks(undefined).map((s) => s[2])).toEqual(["unknown", "unknown"]);
	});

	it("counts the steps after the one it's on", () => {
		expect(stepsLeft(creating(1, 5, "Cloning"))).toBe(3);
		expect(stepsLeft(creating(4, 5, "Checking"))).toBe(0);
		expect(stepsLeft({ state: "running" })).toBe(0);
	});
});
