// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { setLanguageChoice } from "../i18n";
import type { Workspace } from "../bindings";
import { mockSource } from "../data/mock-source";
import { MHQ_WORKSPACE } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import { rollbackLoses, unsavedOf, unsavedText } from "./unsaved";

const workspace = async (): Promise<Workspace> =>
	applySnapshot(
		await mockSource({ now: () => NOW, tick: false }).snapshot()
	).tables.workspace.get(MHQ_WORKSPACE)!;

describe("unsaved work", () => {
	it("says what's unsaved in words", () => {
		const read_at = NOW;
		expect(unsavedText({ changed: 3, commits: 1, no_upstream: 0, read_at })).toBe(
			"3 changed files and 1 unpushed commit"
		);
		expect(unsavedText({ changed: 0, commits: 0, no_upstream: 2, read_at })).toBe(
			"2 branches with no upstream"
		);
		expect(unsavedText({ changed: 0, commits: 0, no_upstream: 0, read_at })).toBe(
			"Nothing unsaved"
		);
	});

	it("uses translated plural forms for branches without an upstream", () => {
		setLanguageChoice("de");
		try {
			expect(unsavedText({ changed: 0, commits: 0, no_upstream: 2, read_at: NOW })).toBe(
				"2 Branches ohne Upstream"
			);
		} finally {
			setLanguageChoice("en");
		}
	});

	it("reads a running workspace's summary as fresh", async () => {
		const view = unsavedOf(await workspace(), NOW)!;
		expect(view).toMatchObject({ any: true, count: 4, old: false });
		expect(view.text).toBe("3 changed files and 1 unpushed commit");
		expect(view.readAt).toMatch(/^Read at /);
	});

	it("marks a stopped workspace's summary old", async () => {
		const running = await workspace();
		const stopped: Workspace = {
			...running,
			managed: { ...running.managed!, state: { state: "stopped" } },
		};
		const view = unsavedOf(stopped, NOW)!;
		expect(view.old).toBe(true);
		expect(view.readAt).toMatch(/^Last read at /);
	});

	it("has nothing to say before the guest reads it", async () => {
		const running = await workspace();
		expect(
			unsavedOf({ ...running, managed: { ...running.managed!, unsaved: null } }, NOW)
		).toBeNull();
		expect(unsavedOf({ ...running, managed: null }, NOW)).toBeNull();
	});

	it("asks before a roll back only when it would lose something", async () => {
		const running = await workspace();
		const clean = {
			...running,
			managed: {
				...running.managed!,
				unsaved: { changed: 0, commits: 0, no_upstream: 0, read_at: NOW },
			},
		};
		expect(rollbackLoses(running, false)).toBe("3 changed files and 1 unpushed commit");
		expect(rollbackLoses(running, true)).toBeNull();
		expect(rollbackLoses(clean, false)).toBeNull();
		const unread = { ...running, managed: { ...running.managed!, unsaved: null } };
		expect(rollbackLoses(unread, false)).toMatch(/hasn't heard/);
		expect(rollbackLoses(unread, true)).toBeNull();
	});
});
