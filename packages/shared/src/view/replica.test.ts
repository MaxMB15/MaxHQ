// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Delta, Entity, Snapshot } from "../bindings";

import { EMPTY_VIEW, all, applyDelta, applySnapshot, lookup } from "./replica";

const project = (id: string, name: string): Entity => ({
	entity: "project",
	id,
	name,
	repo: null,
	default_vendor: null,
	default_account: null,
	default_workspace: null,
});

const device = (id: string): Entity => ({
	entity: "device",
	id,
	name: id,
	os: "linux",
	boot_id: "b1",
	maxd_version: "0.1.0",
	winding_down_until: null,
	workspace_host: null,
});

const snapshot = (seq: number, entities: Entity[]): Snapshot => ({ seq, entities });

/** An entity of a kind from a newer core, which this build doesn't hold. */
const hologram = { entity: "hologram", id: "h1", beam: 3 } as unknown as Entity;

describe("kinds this build doesn't hold", () => {
	it("leaves them out of a snapshot and keeps the rest", () => {
		const view = applySnapshot(snapshot(3, [hologram, project("p1", "MaxHQ")]));
		expect(all(view, "project").map((p) => p.name)).toEqual(["MaxHQ"]);
		expect(Object.keys(view.tables)).not.toContain("hologram");
	});

	it("leaves them out of a delta and applies the rest", () => {
		const base = applySnapshot(snapshot(5, [project("p1", "MaxHQ")]));
		const changes = [
			{ op: "upsert", value: hologram },
			{ op: "remove", key: { entity: "hologram", id: "h1" } },
			{ op: "rename", from: "p1", to: "p2" },
			{ op: "upsert", value: project("p2", "Comet") },
			{ op: "remove", key: { entity: "project", id: "p1" } },
		] as unknown as Delta["changes"];
		const result = applyDelta(base, { seq: 6, changes });
		expect(result.kind).toBe("applied");
		if (result.kind !== "applied") return;
		expect(result.view.seq).toBe(6);
		expect(all(result.view, "project").map((p) => p.name)).toEqual(["Comet"]);
		expect(Object.keys(result.view.tables)).not.toContain("hologram");
	});
});

describe("applySnapshot", () => {
	it("replaces everything and takes the snapshot's version", () => {
		const first = applySnapshot(snapshot(3, [project("p1", "MaxHQ"), device("d1")]));
		expect(first.seq).toBe(3);
		expect(all(first, "project").map((p) => p.name)).toEqual(["MaxHQ"]);

		const second = applySnapshot(snapshot(9, [project("p2", "Comet")]));
		expect(second.seq).toBe(9);
		expect(lookup(second, { entity: "project", id: "p1" })).toBeUndefined();
		expect(lookup(second, { entity: "project", id: "p2" })?.name).toBe("Comet");
		expect(all(second, "device")).toEqual([]);
	});
});

describe("applyDelta", () => {
	const base = applySnapshot(snapshot(5, [project("p1", "MaxHQ"), device("d1")]));

	it("calls a delta before any snapshot a gap", () => {
		expect(applyDelta(EMPTY_VIEW, { seq: 1, changes: [] })).toEqual({
			kind: "gap",
			have: null,
			got: 1,
		});
	});

	it("applies the next version in order", () => {
		const result = applyDelta(base, {
			seq: 6,
			changes: [
				{ op: "upsert", value: project("p1", "Renamed") },
				{ op: "upsert", value: project("p3", "Mosaic") },
				{ op: "remove", key: { entity: "project", id: "p3" } },
				{ op: "upsert", value: project("p4", "AtlasMaps") },
			],
		});
		expect(result.kind).toBe("applied");
		if (result.kind !== "applied") return;
		expect(result.view.seq).toBe(6);
		expect(lookup(result.view, { entity: "project", id: "p1" })?.name).toBe("Renamed");
		expect(lookup(result.view, { entity: "project", id: "p3" })).toBeUndefined();
		expect(lookup(result.view, { entity: "project", id: "p4" })?.name).toBe("AtlasMaps");
	});

	it("leaves the old view alone and keeps untouched tables", () => {
		const result = applyDelta(base, {
			seq: 6,
			changes: [{ op: "remove", key: { entity: "project", id: "p1" } }],
		});
		if (result.kind !== "applied") throw new Error("expected applied");
		expect(lookup(base, { entity: "project", id: "p1" })?.name).toBe("MaxHQ");
		expect(result.view.tables.device).toBe(base.tables.device);
		expect(result.view.tables.project).not.toBe(base.tables.project);
	});

	it("ignores a removal of something it doesn't hold", () => {
		const result = applyDelta(base, {
			seq: 6,
			changes: [{ op: "remove", key: { entity: "agent", id: "gone" } }],
		});
		expect(result.kind).toBe("applied");
	});

	it("ignores a version it already holds", () => {
		expect(applyDelta(base, { seq: 5, changes: [] })).toEqual({ kind: "repeat" });
		expect(applyDelta(base, { seq: 2, changes: [] })).toEqual({ kind: "repeat" });
	});

	it("calls a skipped version a gap", () => {
		expect(applyDelta(base, { seq: 7, changes: [] })).toEqual({ kind: "gap", have: 5, got: 7 });
	});

	it("keys device settings by their device", () => {
		const settings: Entity = {
			entity: "device_settings",
			device: "d1",
			default_permission: "auto",
			workspace_network: null,
			workspace_pool: null,
		};
		const result = applyDelta(base, { seq: 6, changes: [{ op: "upsert", value: settings }] });
		if (result.kind !== "applied") throw new Error("expected applied");
		expect(lookup(result.view, { entity: "device_settings", id: "d1" })).toEqual(settings);
	});

	it("holds and removes a device's ACP agents", () => {
		const agent: Entity = {
			entity: "acp_agent",
			id: "acp-1",
			device: "d1",
			name: "Gemini",
			program: "gemini",
		};
		const added = applyDelta(base, { seq: 6, changes: [{ op: "upsert", value: agent }] });
		if (added.kind !== "applied") throw new Error("expected applied");
		expect(all(added.view, "acp_agent")).toEqual([agent]);
		const gone = applyDelta(added.view, {
			seq: 7,
			changes: [{ op: "remove", key: { entity: "acp_agent", id: "acp-1" } }],
		});
		if (gone.kind !== "applied") throw new Error("expected applied");
		expect(all(gone.view, "acp_agent")).toEqual([]);
	});
});
