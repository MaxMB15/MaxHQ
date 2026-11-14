// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { SETTLED_PAGE, SETTLED_SHOWN, createUiStore } from "./store";

const ORDER = ["a", "b", "c", "d", "e"];

describe("picking agents", () => {
	it("toggles one with Command and picks a range with Shift from the last one picked", () => {
		const ui = createUiStore();
		const { togglePicked, pickTo } = ui.getState();
		togglePicked("b");
		pickTo("d", ORDER);
		expect([...ui.getState().picked]).toEqual(["b", "c", "d"]);
		togglePicked("c");
		expect([...ui.getState().picked].sort()).toEqual(["b", "d"]);
		expect(ui.getState().anchor).toBe("b");
	});

	it("picks from the agent you opened", () => {
		const ui = createUiStore();
		ui.getState().selectAgent("d", false);
		ui.getState().pickTo("b", ORDER);
		expect([...ui.getState().picked].sort()).toEqual(["b", "c", "d"]);
	});

	it("picks one alone when the anchor isn't shown", () => {
		const ui = createUiStore({ anchor: "gone" });
		ui.getState().pickTo("c", ORDER);
		expect([...ui.getState().picked]).toEqual(["c"]);
		expect(ui.getState().anchor).toBe("c");
	});

	it("ends when you open an agent or clear it", () => {
		const ui = createUiStore();
		ui.getState().togglePicked("a");
		ui.getState().selectAgent("e", false);
		expect(ui.getState().picked.size).toBe(0);
		ui.getState().togglePicked("a");
		ui.getState().clearPicked();
		expect(ui.getState().picked.size).toBe(0);
		expect(ui.getState().anchor).toBe("a");
	});

	it("drops the agents a delete took, and any gone from the view, with their anchor", () => {
		const ui = createUiStore();
		for (const agent of ["a", "b", "c", "d"]) ui.getState().togglePicked(agent);
		ui.getState().unpickDeleted(new Set(["b", "d"]), (agent) => agent !== "c");
		expect([...ui.getState().picked]).toEqual(["a"]);
		expect(ui.getState().anchor).toBeNull();

		const before = ui.getState();
		ui.getState().unpickDeleted(new Set(["e"]), () => true);
		expect(ui.getState()).toBe(before);
	});
});

describe("the shelves", () => {
	it("start closed and show settled agents a page at a time", () => {
		const ui = createUiStore();
		expect(ui.getState().openShelves.size).toBe(0);
		ui.getState().toggleShelf("settled");
		expect(ui.getState().openShelves.has("settled")).toBe(true);
		ui.getState().showMoreSettled();
		expect(ui.getState().settledShown).toBe(SETTLED_SHOWN + SETTLED_PAGE);
	});
});

describe("the Pulls page and its PR", () => {
	it("opens a project's page, which opening an agent leaves, and a PR beside it", () => {
		const ui = createUiStore({ mode: "devices" });
		ui.getState().openPulls("proj-a");
		expect(ui.getState()).toMatchObject({ pullsOf: "proj-a", mode: "projects" });
		const pull = { key: { repo: "owner/repo", number: 7 }, device: "dev-a" };
		ui.getState().openPull(pull);
		ui.getState().selectAgent("a", false);
		expect(ui.getState()).toMatchObject({ pullsOf: null, shownPull: pull });
		ui.getState().closePull();
		expect(ui.getState().shownPull).toBeNull();
	});
});
