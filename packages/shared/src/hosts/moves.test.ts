// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { MOCK_MOVE } from "../data/mock-hosts";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import type { ViewState } from "../view/replica";
import {
	isOver,
	movableContainers,
	moveActions,
	moveNote,
	moveOf,
	movesOn,
	moveSteps,
	offersMoves,
	retireLoses,
	STEP_STATUS_LABEL,
} from "./moves";

const load = async () =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());

describe("moves", () => {
	it("lists a device's moves and finds one by its container", async () => {
		const view = await load();
		expect(movesOn(view, DEVBOX).map((m) => m.id)).toContain(MOCK_MOVE);
		expect(moveOf(view, DEVBOX, "ws-alpha")?.id).toBe(MOCK_MOVE);
		expect(offersMoves(view, DEVBOX)).toBe(true);
		expect(offersMoves(view, MAC)).toBe(false);
	});

	it("shows each step with what it changes", async () => {
		const move = (await load()).tables.move.get(MOCK_MOVE)!;
		const steps = moveSteps(move);
		expect(steps.map((s) => STEP_STATUS_LABEL[s.status])).toContain("Done");
		expect(steps.map((s) => s.status)).toEqual([
			"done",
			"ready",
			"todo",
			"todo",
			"todo",
			"todo",
		]);
		expect(steps[1]!.changes).toEqual(["Copy ws-alpha to mhq-alpha from a new snapshot"]);
		expect(moveActions(move)).toEqual([{ kind: "run", step: "copy", label: "Copy" }]);
		expect(moveNote(move, NOW)).toBeNull();
	});

	it("offers a copy again before the switch when the original changed", async () => {
		const move = (await load()).tables.move.get("move-bravo")!;
		expect(moveActions(move).map((a) => a.label)).toEqual(["Copy again", "Switch"]);
		expect(moveNote(move, NOW)).toBe("ws-bravo changed since the copy.");
	});

	it("goes back or retires while the original is kept", async () => {
		const move = (await load()).tables.move.get("move-charlie")!;
		expect(moveActions(move).map((a) => a.kind)).toEqual(["move_back", "retire"]);
		expect(moveNote(move, NOW)).toMatch(/^ws-charlie is kept until /);
	});

	it("tries a failed step again with its reason", async () => {
		const move = (await load()).tables.move.get("move-delta")!;
		expect(moveSteps(move)[2]).toMatchObject({
			status: "failed",
			note: "The guest agent didn't answer within 2 minutes.",
		});
		expect(moveActions(move)).toEqual([{ kind: "run", step: "converge", label: "Try again" }]);
	});

	it("offers nothing once a move is over", async () => {
		const move = (await load()).tables.move.get("move-echo")!;
		expect(isOver(move)).toBe(true);
		expect(moveActions(move)).toEqual([]);
		expect(moveNote(move, NOW)).toBe("Moved back. ws-echo runs again.");
		const retired = { ...move, step: "retire" as const, state: { state: "retired" as const } };
		expect(moveSteps(retired).every((s) => s.status === "done")).toBe(true);
		expect(moveNote(retired, NOW)).toBe("Moved. ws-echo is deleted.");
	});

	it("offers a move only for containers MaxHQ didn't make and that aren't moving", async () => {
		const view = await load();
		const ids = (v: ViewState) => movableContainers(v, DEVBOX).map((w) => w.id);
		const before = ids(view);
		expect(before).not.toContain("ws-alpha");
		expect(before).not.toContain("ws-bravo");
		const tables = { ...view.tables, move: new Map(view.tables.move) };
		tables.move.delete("move-bravo");
		const fewer: ViewState = { ...view, tables };
		expect(new Set(ids(fewer))).toEqual(new Set([...before, "ws-bravo"]));
		expect(ids(fewer)).not.toContain("ws-mhq-maxhq");
	});

	it("says what retiring the original loses", async () => {
		const view = await load();
		const move = view.tables.move.get(MOCK_MOVE)!;
		expect(retireLoses(view, move, NOW)).toMatch(/can't read unsaved work in ws-alpha/);
		const original = view.tables.workspace.get("ws-alpha")!;
		const read = (changed: number): ViewState => {
			const found = {
				...original,
				managed: {
					...view.tables.workspace.get("ws-mhq-maxhq")!.managed!,
					unsaved: { changed, commits: 0, no_upstream: 0, read_at: NOW },
				},
			};
			const workspace = new Map(view.tables.workspace).set("ws-alpha", found);
			return { ...view, tables: { ...view.tables, workspace } };
		};
		expect(retireLoses(read(2), move, NOW)).toBe(
			"ws-alpha has 2 changed files. Deleting it loses that."
		);
		expect(retireLoses(read(0), move, NOW)).toBe("ws-alpha has nothing unsaved.");
	});
});
