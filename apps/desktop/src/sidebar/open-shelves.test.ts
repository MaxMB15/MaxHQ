// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import type { Services } from "@/app/services";
import { createUiStore } from "@maxhq/shared/ui/store";

import { loadShelves, readShelves, SHELVES_KEY, watchShelves } from "./open-shelves";

const saved = (open: unknown) => JSON.stringify({ version: 1, open });
// Each test gets its own window.
const services = () => ({ ui: createUiStore() }) as unknown as Services;

afterEach(() => {
	vi.restoreAllMocks();
});

describe("open shelves", () => {
	it("read as every shelf closed when nothing was saved or it can't be read", () => {
		for (const raw of [
			null,
			"{not json",
			"[]",
			JSON.stringify({ version: 9, open: ["settled"] }),
			saved("settled"),
		]) {
			expect(readShelves(raw)).toEqual(new Set());
		}
	});

	it("keep the saved shelves they know", () => {
		expect(readShelves(saved(["settled", "attic", 3, "working"]))).toEqual(
			new Set(["working", "settled"])
		);
	});

	it("save each shelf you open or close and give them back next time", () => {
		const one = services();
		const stop = watchShelves(one);
		one.ui.getState().toggleShelf("settled");
		one.ui.getState().toggleShelf("snoozed");
		one.ui.getState().toggleShelf("snoozed");
		expect(loadShelves()).toEqual(new Set(["settled"]));

		stop();
		one.ui.getState().toggleShelf("working");
		expect(loadShelves()).toEqual(new Set(["settled"]));
	});

	it("keep working when storage throws", () => {
		vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
			throw new Error("blocked");
		});
		vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
			throw new Error("full");
		});
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		expect(loadShelves()).toEqual(new Set());
		const one = services();
		const stop = watchShelves(one);
		expect(() => one.ui.getState().toggleShelf("working")).not.toThrow();
		expect(one.ui.getState().openShelves).toEqual(new Set(["working"]));
		expect(warn).toHaveBeenCalled();
		stop();
	});

	it("take up what another window saves", () => {
		const one = services();
		const stop = watchShelves(one);
		window.dispatchEvent(
			new StorageEvent("storage", { key: SHELVES_KEY, newValue: saved(["snoozed"]) })
		);
		expect(one.ui.getState().openShelves).toEqual(new Set(["snoozed"]));
		stop();
		window.dispatchEvent(new StorageEvent("storage", { key: SHELVES_KEY, newValue: null }));
		expect(one.ui.getState().openShelves).toEqual(new Set(["snoozed"]));
	});
});
