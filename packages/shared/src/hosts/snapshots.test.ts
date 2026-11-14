// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC, MHQ_WORKSPACE } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	clearSnapshots,
	offersSnapshots,
	refreshSnapshots,
	restoreSnapshot,
	snapshotsOf,
} from "./snapshots";

const source = () => mockSource({ now: () => NOW, tick: false });

afterEach(() => {
	clearSnapshots();
	vi.useRealTimers();
});

describe("snapshots", () => {
	it("knows which devices keep snapshots", async () => {
		const view = applySnapshot(await source().snapshot());
		expect(offersSnapshots(view, DEVBOX)).toBe(true);
		expect(offersSnapshots(view, MAC)).toBe(false);
	});

	it("lists a workspace's snapshots", async () => {
		await refreshSnapshots(source(), MHQ_WORKSPACE);
		const list = snapshotsOf(MHQ_WORKSPACE);
		expect(list?.state).toBe("done");
		expect(list?.snapshots?.map((s) => s.name)).toContain("snap-before-move");
	});

	it("keeps the last list on show when a refresh fails", async () => {
		await refreshSnapshots(source(), MHQ_WORKSPACE);
		const failing = {
			listSnapshots: async () => ({
				status: "error" as const,
				error: { kind: "failed" as const, message: "devbox didn't answer" },
			}),
		};
		await refreshSnapshots(failing, MHQ_WORKSPACE);
		const list = snapshotsOf(MHQ_WORKSPACE);
		expect(list?.state).toBe("failed");
		expect(list?.snapshots).toHaveLength(3);
	});

	it("lists again after a restore, and throws on a refusal", async () => {
		vi.useFakeTimers();
		const mock = source();
		await expect(restoreSnapshot(mock, MHQ_WORKSPACE, "snap-daily-1", false)).rejects.toThrow(
			/Rolling back loses this: 3 changed files and 1 unpushed commit/
		);
		const restoring = restoreSnapshot(mock, MHQ_WORKSPACE, "snap-daily-1", false, true);
		await vi.runAllTimersAsync();
		await restoring;
		expect(snapshotsOf(MHQ_WORKSPACE)?.state).toBe("done");
		await expect(restoreSnapshot(mock, MHQ_WORKSPACE, "snap-gone", true)).rejects.toThrow();
	});

	it("has no list for a workspace never asked about", () => {
		expect(snapshotsOf("ws-alpha")).toBeUndefined();
	});
});
