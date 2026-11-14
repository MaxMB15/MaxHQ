// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import type { Services } from "@/app/services";

import {
	clampWidth,
	DEFAULT_PREFS,
	DEFAULT_WIDTH,
	loadPrefs,
	MAX_WIDTH,
	MIN_WIDTH,
	PREFS_KEY,
	prefsOf,
	readPrefs,
	setPrefs,
	watchPrefs,
} from "./prefs";

const AFTER = { kind: "after", minutes: 30 } as const;
const saved = (prefs: object) => JSON.stringify({ version: 1, ...prefs });
// Each test gets its own window's settings.
const services = () => ({}) as Services;

afterEach(() => {
	vi.restoreAllMocks();
});

describe("sidebar settings", () => {
	it("read as the defaults when nothing was saved or it can't be read", () => {
		for (const raw of [
			null,
			"{not json",
			"[]",
			JSON.stringify({ version: 9, workingShelf: true }),
		]) {
			expect(readPrefs(raw)).toEqual(DEFAULT_PREFS);
		}
	});

	it("start with the Working shelf off, and keep it on once you turn it on", () => {
		expect(loadPrefs().workingShelf).toBe(false);
		expect(readPrefs(saved({ workingShelf: true })).workingShelf).toBe(true);
	});

	it("keep each saved part that checks out and default the rest", () => {
		expect(readPrefs(saved({ workingShelf: true, snoozePresets: "soon" }))).toEqual({
			...DEFAULT_PREFS,
			workingShelf: true,
		});
		expect(readPrefs(saved({ workingShelf: "yes", snoozePresets: [AFTER] }))).toEqual({
			...DEFAULT_PREFS,
			snoozePresets: [AFTER],
		});
		expect(readPrefs(saved({ open: "no", width: "wide" }))).toEqual(DEFAULT_PREFS);
		const nine = Array.from({ length: 9 }, () => AFTER);
		expect(readPrefs(saved({ snoozePresets: nine })).snoozePresets).toEqual(
			DEFAULT_PREFS.snoozePresets
		);
		expect(
			readPrefs(saved({ snoozePresets: [{ kind: "after", minutes: -5 }] })).snoozePresets
		).toEqual(DEFAULT_PREFS.snoozePresets);
	});

	it("start with the sidebar open at its default width", () => {
		expect(loadPrefs()).toMatchObject({ open: true, width: DEFAULT_WIDTH });
		expect(readPrefs(saved({ open: false, width: 300 }))).toMatchObject({
			open: false,
			width: 300,
		});
	});

	it("hold the width between the narrowest and the widest", () => {
		expect(clampWidth(100)).toBe(MIN_WIDTH);
		expect(clampWidth(9000)).toBe(MAX_WIDTH);
		expect(clampWidth(300.4)).toBe(300);
		expect(readPrefs(saved({ width: 40 })).width).toBe(MIN_WIDTH);
		expect(readPrefs(saved({ width: 1200 })).width).toBe(MAX_WIDTH);
	});

	it("save each change and give it back next time", () => {
		const one = services();
		setPrefs(one, { workingShelf: true });
		expect(prefsOf(one).getState().workingShelf).toBe(true);
		expect(loadPrefs()).toEqual({ ...DEFAULT_PREFS, workingShelf: true });
		expect(prefsOf(services()).getState().workingShelf).toBe(true);
	});

	it("keep working when storage throws", () => {
		vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
			throw new Error("blocked");
		});
		vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
			throw new Error("full");
		});
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		expect(loadPrefs()).toEqual(DEFAULT_PREFS);
		const one = services();
		expect(() => setPrefs(one, { snoozePresets: [AFTER] })).not.toThrow();
		expect(prefsOf(one).getState().snoozePresets).toEqual([AFTER]);
		expect(warn).toHaveBeenCalled();
	});

	it("take up what another window saves", () => {
		const one = services();
		const stop = watchPrefs(one);
		window.dispatchEvent(
			new StorageEvent("storage", {
				key: PREFS_KEY,
				newValue: saved({ workingShelf: true }),
			})
		);
		expect(prefsOf(one).getState().workingShelf).toBe(true);
		stop();
		window.dispatchEvent(new StorageEvent("storage", { key: PREFS_KEY, newValue: null }));
		expect(prefsOf(one).getState().workingShelf).toBe(true);
	});

	it("keep this window's open state and width when another window saves", () => {
		const one = services();
		setPrefs(one, { open: false, width: 400 });
		const stop = watchPrefs(one);
		window.dispatchEvent(
			new StorageEvent("storage", {
				key: PREFS_KEY,
				newValue: saved({ workingShelf: true, open: true, width: 240 }),
			})
		);
		expect(prefsOf(one).getState()).toMatchObject({
			workingShelf: true,
			open: false,
			width: 400,
		});
		stop();
	});
});
