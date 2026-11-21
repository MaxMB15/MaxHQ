// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { beforeEach, describe, expect, it, vi } from "vitest";

/** The guard as a new page has it. */
const fresh = async () => {
	vi.resetModules();
	return (await import("./hang-guard")).hangGuard("test");
};

/** A new launch, whose window had `stopped` when the app last quit. */
const launch = (stopped: boolean | undefined) => {
	sessionStorage.clear();
	window.__MAXHQ_LAST_STOPPED__ = stopped;
};

beforeEach(() => {
	localStorage.clear();
	launch(undefined);
});

describe("a hang guard", () => {
	it("holds back a mark still running when the window stopped", async () => {
		const guard = await fresh();
		guard.start("a");
		guard.start("b");
		guard.finish("b");
		launch(true);
		const next = await fresh();
		expect(next.held("a")).toBe(true);
		expect(next.held("b")).toBe(false);
		next.allow("a");
		expect(next.held("a")).toBe(false);
	});

	it("holds back when nothing says how the window went away", async () => {
		(await fresh()).start("a");
		launch(undefined);
		expect((await fresh()).held("a")).toBe(true);
	});

	it("runs again what ran when the app quit with its window answering", async () => {
		(await fresh()).start("a");
		launch(false);
		const next = await fresh();
		expect(next.held("a")).toBe(false);
		expect(localStorage.getItem("maxhq.test.drawing")).toBeNull();
	});

	it("holds back on a page after the launch's first, which can't know", async () => {
		launch(false);
		(await fresh()).start("a");
		expect((await fresh()).held("a")).toBe(true);
	});

	it("ends its runs when the page goes away on its own", async () => {
		const guard = await fresh();
		guard.start("a");
		// Kept for going back to, the page still runs.
		const kept = Object.assign(new Event("pagehide"), { persisted: true });
		window.dispatchEvent(kept);
		expect(localStorage.getItem("maxhq.test.drawing")).toBe("a");
		window.dispatchEvent(new Event("pagehide"));
		expect(localStorage.getItem("maxhq.test.drawing")).toBeNull();
	});
});
