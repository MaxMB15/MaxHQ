// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { NoteVersion, NoteVersionInfo } from "@maxhq/shared/bindings";

import { loaded, pickVersion, readHistory, readVersion, stepBack } from "./history";

const info: NoteVersionInfo = {
	hash: "h2",
	parents: ["h1"],
	title: "Plan",
	deleted: false,
	at: 1_000,
	by: null,
	size: 4,
};

const version: NoteVersion = {
	hash: "h1",
	project: "p-a",
	note: "n1",
	parents: [],
	title: "Plan",
	body: "Old",
	deleted: false,
	at: 500,
	by: null,
};

describe("loaded", () => {
	it("takes an answer's data", () => {
		expect(loaded({ status: "ok", data: [info] })).toEqual({ kind: "read", data: [info] });
	});

	it("says why a read failed", () => {
		expect(
			loaded({ status: "error", error: { kind: "not_connected", device: "devbox" } })
		).toEqual({ kind: "failed", reason: "devbox isn't connected." });
		expect(
			loaded({ status: "error", error: { kind: "failed", message: "Disk full" } })
		).toEqual({ kind: "failed", reason: "Disk full" });
	});
});

describe("readHistory", () => {
	it("asks the device for the note's versions", async () => {
		const noteHistory = vi.fn(async () => ({ status: "ok" as const, data: [info] }));
		expect(await readHistory({ noteHistory }, "dev-box", "p-a", "n1")).toEqual({
			kind: "read",
			data: [info],
		});
		expect(noteHistory).toHaveBeenCalledWith("dev-box", "p-a", "n1");
	});

	it("says why when the read throws", async () => {
		const noteHistory = vi.fn(() => Promise.reject(new Error("Socket closed")));
		expect(await readHistory({ noteHistory }, "dev-box", "p-a", "n1")).toEqual({
			kind: "failed",
			reason: "Socket closed",
		});
	});
});

describe("readVersion", () => {
	it("reads the picked version, not the head", async () => {
		const readNote = vi.fn(async () => ({ status: "ok" as const, data: version }));
		expect(await readVersion({ readNote }, "dev-box", "p-a", "n1", "h1")).toEqual({
			kind: "read",
			data: version,
		});
		expect(readNote).toHaveBeenCalledWith("dev-box", "p-a", "n1", "h1");
	});

	it("says why it couldn't", async () => {
		const readNote = vi.fn(async () => ({
			status: "error" as const,
			error: { kind: "failed" as const, message: "No such version" },
		}));
		expect(await readVersion({ readNote }, "dev-box", "p-a", "n1", "h9")).toEqual({
			kind: "failed",
			reason: "No such version",
		});
	});
});

describe("pickVersion", () => {
	it("goes back to the note for the newest, the current one", () => {
		expect(pickVersion(0, "h2")).toEqual({ kind: "note" });
	});

	it("opens any older one read-only", () => {
		expect(pickVersion(1, "h1")).toEqual({ kind: "version", hash: "h1" });
	});
});

describe("stepBack", () => {
	it("goes from a version to the history, and from the history to the note", () => {
		expect(stepBack({ kind: "version", hash: "h1" })).toEqual({ kind: "history" });
		expect(stepBack({ kind: "history" })).toEqual({ kind: "note" });
	});
});
