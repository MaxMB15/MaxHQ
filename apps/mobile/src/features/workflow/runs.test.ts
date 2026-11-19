// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { LoopState } from "@maxhq/shared/bindings";
import { mockRunEntity, mockRunSnapshot, nodeState } from "@maxhq/shared/data/mock-workflow";
import { NOW } from "@maxhq/shared/test/build";
import { openAtFirst, tabRows } from "@maxhq/shared/workflow/layout";
import { runChip } from "@maxhq/shared/workflow/select";

import { color, tint } from "../../theme";
import {
	barGeometry,
	chipLook,
	GLYPH_TINT,
	loopsByRow,
	nodeRowLabel,
	nodeSheet,
	runLine,
	runMenu,
	timeAxis,
	workflowsEntry,
} from "./runs";

const SETTINGS = mockRunEntity(NOW, "t-20261101-example-settings");
const FILTERS = mockRunEntity(NOW, "t-20261101-example-filters");
const SYNC = mockRunEntity(NOW, "t-20261101-example-sync");

const MINUTE = 60_000;

describe("GLYPH_TINT", () => {
	it("draws each status in the desktop's color", () => {
		expect(GLYPH_TINT.warn).toBe(color.busy);
		expect(GLYPH_TINT.good).toBe(color.ready);
		expect(GLYPH_TINT.bad).toBe(color.error);
		expect(GLYPH_TINT.accent).toBe(color.primary);
	});
});

describe("chipLook", () => {
	it("is amber while the run waits on you and quiet once it's done", () => {
		expect(chipLook(runChip(SETTINGS))).toEqual({
			border: tint(color.busy, 0.4),
			text: color.busyText,
		});
		expect(chipLook(runChip(SYNC)).text).toBe(color.mutedForeground);
		expect(chipLook(runChip(FILTERS)).text).toBe(color.secondaryForeground);
	});
});

describe("runLine", () => {
	it("shows a live run's progress and that it waits on you", () => {
		const line = runLine(SETTINGS, NOW);
		expect(line).toMatchObject({
			status: "waiting_human",
			label: "Settings page redesign",
			workflow: SETTINGS.key.workflow,
			side: `${SETTINGS.progress.done}/${SETTINGS.progress.total}`,
			needsYou: true,
		});
		expect(line.spoken).toMatch(/^Settings page redesign, \d+ of \d+ done, needs you$/);
	});

	it("shows how long ago a finished run ended", () => {
		expect(runLine(SYNC, NOW)).toMatchObject({ status: "done", side: "15m", needsYou: false });
		expect(runLine({ ...SYNC, last_activity: null }, NOW).side).toBe("");
	});
});

describe("workflowsEntry", () => {
	it("counts the runs and shows the liveliest live one", () => {
		expect(workflowsEntry([FILTERS, SETTINGS, SYNC])).toEqual({
			count: 3,
			status: "waiting_human",
		});
		expect(workflowsEntry([FILTERS, SYNC]).status).toBe("busy");
	});

	it("shows no state once every run is done", () => {
		expect(workflowsEntry([SYNC])).toEqual({ count: 1, status: null });
		expect(workflowsEntry([])).toEqual({ count: 0, status: null });
	});
});

describe("runMenu", () => {
	it("offers Open, Stop run and Copy run ID", () => {
		const open = vi.fn();
		const stop = vi.fn();
		const copy = vi.fn();
		const menu = runMenu({ open, stop, copy });
		expect(menu.map((choice) => choice.label)).toEqual(["Open", "Stop run…", "Copy run ID"]);
		expect(menu[1]?.destructive).toBe(true);
		menu[1]?.pick();
		expect(stop).toHaveBeenCalledOnce();
	});

	it("leaves out Stop run when nothing works on the run, and Open on the run's own screen", () => {
		const menu = runMenu({ stop: null, copy: () => {} });
		expect(menu.map((choice) => choice.label)).toEqual(["Copy run ID"]);
	});
});

describe("loopsByRow", () => {
	const run = mockRunSnapshot(NOW, "t-20261101-example-sync");
	const status = (id: string) => run.nodes.find((node) => node.id === id)?.status;

	it("puts each loop that ran on the row it went back from", () => {
		const rows = tabRows(run.shape, status, new Set(run.shape.groups.map((g) => g.id)));
		const byRow = loopsByRow(rows, run.loops);
		const shown = [...byRow.values()].flat();
		expect(shown).toHaveLength(run.loops.filter((loop) => loop.count > 0).length);
		for (const [row, loops] of byRow) {
			const at = rows[row]!;
			for (const loop of loops) {
				expect(at.kind === "node" ? at.node.id : at.members.map((m) => m.id)).toContain(
					loop.from
				);
			}
		}
	});

	it("puts a member's loop on its folded group's row, and leaves out loops that didn't run", () => {
		const rows = tabRows(run.shape, status, openAtFirst(run.shape));
		const loop: LoopState = { from: "a", to: "a", label: null, count: 0, budget: 3 };
		expect(loopsByRow(rows, [loop]).size).toBe(0);
		const folded = rows.find((row) => row.kind === "group" && !row.open);
		expect(folded?.kind).toBe("group");
		if (folded?.kind !== "group") return;
		const member = folded.members[0]!.id;
		const byRow = loopsByRow(rows, [{ ...loop, from: member, count: 1 }]);
		expect([...byRow.keys()]).toEqual([rows.indexOf(folded)]);
	});
});

describe("nodeRowLabel", () => {
	const shape = { id: "a", label: "Review", group: null, lane: null, host: null };

	it("says the node, how it stands, its loops and that it opens its agent", () => {
		const node = nodeState("a", { status: "failed" });
		expect(nodeRowLabel(shape, node, null)).toBe("Review, Failed");
		expect(
			nodeRowLabel(shape, node, "agent-a", [
				{ from: "a", to: "b", label: "review", count: 2, budget: 3 },
			])
		).toBe("Review, Failed, review went round 2/3, opens its agent");
	});
});

describe("nodeSheet", () => {
	it("lists the node's agents the phone has, latest first, once each", () => {
		const node = {
			...nodeState("a", { status: "done", note: "Looks good" }),
			badges: [{ text: "round 2", tone: "info" as const }],
			agents: ["agent-b"],
			attempts: [
				{ start: NOW - 50, end: NOW - 40, end_status: "failed" as const, agent: "agent-a" },
				{ start: NOW - 30, end: NOW - 20, end_status: "done" as const, agent: "agent-b" },
				{ start: NOW - 10, end: NOW, end_status: "done" as const, agent: "agent-gone" },
			],
		};
		const titles: Record<string, string> = { "agent-a": "First try", "agent-b": "Second try" };
		const sheet = nodeSheet(node, (id) => titles[id]);
		expect(sheet.agents).toEqual([
			{ id: "agent-b", title: "Second try" },
			{ id: "agent-a", title: "First try" },
		]);
		expect(sheet.message).toBe("Done\nround 2\nLooks good");
	});
});

describe("timeAxis", () => {
	const start = Date.UTC(2026, 10, 1, 9, 3, 20);

	it("widens to whole steps of local time with a tick on each", () => {
		const axis = timeAxis(start, start + 25 * MINUTE, 300, 0);
		expect(axis.ticks[0]).toBe(Date.UTC(2026, 10, 1, 9, 0));
		expect(axis.ticks.at(-1)).toBe(Date.UTC(2026, 10, 1, 9, 30));
		expect(axis.ticks).toHaveLength(4);
		expect(axis.x(axis.ticks[0]!)).toBe(0);
		expect(axis.x(axis.ticks.at(-1)!)).toBe(300);
		expect(axis.x(start)).toBeGreaterThan(0);
	});

	it("lines the steps up with local time", () => {
		const axis = timeAxis(start, start + 5 * 60 * MINUTE, 200, 30 * MINUTE);
		for (const tick of axis.ticks) expect((tick + 30 * MINUTE) % (3 * 60 * MINUTE)).toBe(0);
	});

	it("is a minute long at least, so a run that just started has an axis", () => {
		const axis = timeAxis(start, start, 300, 0);
		expect(axis.ticks.length).toBeGreaterThanOrEqual(2);
		expect(axis.x(start)).toBeGreaterThanOrEqual(0);
		expect(axis.x(start)).toBeLessThan(300);
	});

	it("reads ticks as clock times, or as dates for steps of a day", () => {
		expect(timeAxis(start, start + 25 * MINUTE, 300, 0).label(start)).toMatch(/^\d\d:\d\d$/);
		expect(timeAxis(start, start + 9 * 24 * 60 * MINUTE, 200, 0).label(start)).not.toMatch(
			/^\d\d:\d\d$/
		);
	});
});

describe("barGeometry", () => {
	const x = (at: number) => at / 10;
	const bar = {
		start: 100,
		end: 600,
		running: true,
		status: "busy" as const,
		waitFrom: null,
		agent: null,
	};

	it("is solid all along when it doesn't wait", () => {
		expect(barGeometry(bar, x)).toEqual({ left: 10, width: 50, solid: 50 });
	});

	it("is solid up to where the wait began", () => {
		expect(barGeometry({ ...bar, waitFrom: 400 }, x)).toEqual({
			left: 10,
			width: 50,
			solid: 30,
		});
	});

	it("is 3 points wide at least", () => {
		expect(barGeometry({ ...bar, end: 100 }, x).width).toBe(3);
	});
});
