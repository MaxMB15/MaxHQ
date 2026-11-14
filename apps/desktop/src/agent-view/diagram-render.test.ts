// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// These run the real Mermaid, so a new version that walks dates another way
// fails here and not in someone's feed.

import { beforeEach, describe, expect, it, vi } from "vitest";

import globals from "@/styles/globals.css?raw";

const DRAWING_KEY = "maxhq.diagram.drawing";
const HELD_BACK_KEY = "maxhq.diagram.heldBack";

// Each test gets its own copy, since a window only looks for a leftover once.
const fresh = () => {
	vi.resetModules();
	return import("./diagram-render");
};

const gantt = (...tasks: string[]) =>
	["gantt", "dateFormat YYYY-MM-DD", "excludes 1999-01-01", "section a", ...tasks].join("\n");

// Mermaid works out its colors from the theme.
for (const [, name, value] of globals.matchAll(/(--[\w-]+):\s*([^;]+);/g))
	if (name && value) document.documentElement.style.setProperty(name, value);

beforeEach(() => localStorage.clear());

describe("a chart that covers too long a time", () => {
	it.each([
		["one long task", gantt("t: 2000-01-01, 250000y")],
		[
			"many long tasks",
			gantt(...Array.from({ length: 40 }, (_, index) => `t${index}: 2000-01-01, 400y`)),
		],
	])(
		"fails in good time for %s",
		async (_name, source) => {
			const { drawDiagram } = await fresh();
			// Mermaid loads on the first draw, which takes a while on a busy
			// machine, so a small diagram loads it before the clock starts.
			await drawDiagram("flowchart LR\n a --> b");
			const started = performance.now();
			const drawing = await drawDiagram(source);
			expect(drawing).toEqual({
				state: "failed",
				reason: "The chart covers too long a time to draw.",
			});
			expect(performance.now() - started).toBeLessThan(10_000);
		},
		40_000
	);

	it("leaves nothing behind to hold it back", async () => {
		const { drawDiagram } = await fresh();
		await drawDiagram(gantt("t: 2000-01-01, 250000y"));
		expect(localStorage.getItem(DRAWING_KEY)).toBeNull();
		expect(localStorage.getItem(HELD_BACK_KEY)).toBeNull();
	}, 40_000);

	it("doesn't count against the next diagram", async () => {
		const { drawDiagram } = await fresh();
		await drawDiagram(gantt("t: 2000-01-01, 250000y"));
		const next = await drawDiagram(gantt("t: 2000-01-01, 30d"));
		expect(next.state === "failed" ? next.reason : "").not.toContain("too long a time");
	}, 40_000);
});

describe("a diagram that was drawing when the window went away", () => {
	const source = "flowchart LR\n a --> b";

	/** What the mark for this source reads as, from a draw that's cut short. */
	const markFor = async (text: string): Promise<string> => {
		const { fnv1a } = await import("@maxhq/shared/lib/fnv");
		return fnv1a(text).toString(36);
	};

	it("waits for a click, and a click lets it try", async () => {
		localStorage.setItem(DRAWING_KEY, await markFor(source));
		const { allowDiagram, drawDiagram, drawnDiagram } = await fresh();
		const held = await drawDiagram(source);
		expect(held).toMatchObject({ state: "failed", heldBack: true });
		expect(localStorage.getItem(DRAWING_KEY)).toBeNull();
		expect(localStorage.getItem(HELD_BACK_KEY)).toBe(await markFor(source));

		allowDiagram(source);
		expect(drawnDiagram(source)).toBeUndefined();
		expect(localStorage.getItem(HELD_BACK_KEY)).toBe("");
		const again = await drawDiagram(source);
		expect(again.state === "failed" && again.heldBack).toBeFalsy();
	}, 40_000);

	it("stays held back the next time too", async () => {
		localStorage.setItem(DRAWING_KEY, await markFor(source));
		await (await fresh()).drawDiagram(source);
		const later = await (await fresh()).drawDiagram(source);
		expect(later).toMatchObject({ state: "failed", heldBack: true });
	}, 40_000);

	it("doesn't hold back any other diagram", async () => {
		localStorage.setItem(DRAWING_KEY, await markFor(source));
		const other = await (await fresh()).drawDiagram("flowchart LR\n c --> d");
		expect(other.state === "failed" && other.heldBack).toBeFalsy();
	}, 40_000);

	it("keeps only so many", async () => {
		localStorage.setItem(
			HELD_BACK_KEY,
			Array.from({ length: 30 }, (_, i) => `m${i}`).join(" ")
		);
		localStorage.setItem(DRAWING_KEY, await markFor(source));
		await (await fresh()).drawDiagram(source);
		const kept = localStorage.getItem(HELD_BACK_KEY)?.split(" ") ?? [];
		expect(kept).toHaveLength(20);
		expect(kept.at(-1)).toBe(await markFor(source));
	}, 40_000);
});

describe("a diagram in each theme", () => {
	const source = "flowchart LR\n a --> b";

	// The theme module from the same fresh copy as the renderer.
	const freshPair = async () => {
		const render = await fresh();
		return { ...render, theme: await import("@/app/theme") };
	};

	it("is drawn again in the other theme, and each drawing is kept", async () => {
		const { drawDiagram, drawnDiagram, theme } = await freshPair();
		theme.setThemeChoice("dark");
		const dark = await drawDiagram(source);
		theme.setThemeChoice("light");
		expect(drawnDiagram(source)).toBeUndefined();
		const light = await drawDiagram(source);
		expect(light).not.toBe(dark);
		theme.setThemeChoice("dark");
		expect(drawnDiagram(source)).toBe(dark);
	}, 40_000);

	it("isn't kept for a theme it changed away from while drawing", async () => {
		const { drawDiagram, drawnDiagram, theme } = await freshPair();
		theme.setThemeChoice("dark");
		const drawn = drawDiagram(source);
		theme.setThemeChoice("light");
		await drawn;
		theme.setThemeChoice("dark");
		expect(drawnDiagram(source)).toBeUndefined();
	}, 40_000);
});

it("draws each diagram in a task of its own, so the window paints between two", async () => {
	const order: string[] = [];
	vi.doMock("mermaid", () => ({
		default: {
			initialize: () => {},
			render: async (_id: string, text: string) => {
				order.push(text);
				// What the window has to do next, such as paint.
				if (text === "first") setTimeout(() => order.push("task"));
				return { svg: '<svg viewBox="0 0 10 10"></svg>' };
			},
		},
	}));
	try {
		const { drawDiagram } = await fresh();
		await Promise.all([drawDiagram("first"), drawDiagram("second")]);
		expect(order).toEqual(["first", "task", "second"]);
	} finally {
		vi.doUnmock("mermaid");
	}
});

it("works without storage", async () => {
	const getItem = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
		throw new Error("no storage");
	});
	const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
		throw new Error("no storage");
	});
	const { drawDiagram } = await fresh();
	const drawing = await drawDiagram("flowchart LR\n a --> b");
	expect(drawing.state === "failed" && drawing.heldBack).toBeFalsy();
	getItem.mockRestore();
	setItem.mockRestore();
}, 40_000);
