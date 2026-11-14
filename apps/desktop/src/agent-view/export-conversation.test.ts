// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { InvokeArgs } from "@tauri-apps/api/core";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { copyWhenReady, EXPORT_FOLDER_KEY, saveWhenReady } from "./export-conversation";

const clipboard = (value: object) =>
	Object.defineProperty(navigator, "clipboard", { value, configurable: true });

afterEach(() => {
	Reflect.deleteProperty(navigator, "clipboard");
	vi.unstubAllGlobals();
});

describe("copyWhenReady", () => {
	it("writes the text once it's ready", async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		clipboard({ writeText });
		await expect(copyWhenReady(Promise.resolve("# Chat\n"))).resolves.toEqual({ ok: true });
		expect(writeText).toHaveBeenCalledWith("# Chat\n");
	});

	it("starts the write during the click where the clipboard takes text still to come", async () => {
		const write = vi.fn().mockResolvedValue(undefined);
		clipboard({ write, writeText: vi.fn() });
		const items: Record<string, Promise<Blob>>[] = [];
		vi.stubGlobal(
			"ClipboardItem",
			class {
				constructor(data: Record<string, Promise<Blob>>) {
					items.push(data);
				}
			}
		);
		let finish: (text: string) => void = () => {};
		const done = copyWhenReady(new Promise<string>((resolve) => (finish = resolve)));
		expect(write).toHaveBeenCalledTimes(1);
		finish("# Chat\n");
		await expect(done).resolves.toEqual({ ok: true });
		const blob = await items[0]?.["text/plain"];
		expect(await blob?.text()).toBe("# Chat\n");
	});

	it("gives the reason the conversation didn't load", async () => {
		const writeText = vi.fn();
		clipboard({ writeText });
		await expect(
			copyWhenReady(Promise.reject(new Error("devbox isn't connected.")))
		).resolves.toEqual({ ok: false, reason: "devbox isn't connected." });
		expect(writeText).not.toHaveBeenCalled();
	});

	it("says so when the clipboard says no", async () => {
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		clipboard({ writeText: vi.fn().mockRejectedValue(new Error("Not allowed")) });
		await expect(copyWhenReady(Promise.resolve("text"))).resolves.toEqual({
			ok: false,
			reason: "The clipboard didn't take it. Try again.",
		});
		warn.mockRestore();
	});
});

describe("saveWhenReady", () => {
	const app = globalThis as { isTauri?: boolean };
	const calls: InvokeArgs[] = [];
	// What the save dialog answers next: where the file went, null for
	// cancel, or a failure.
	let answer: unknown;

	beforeEach(() => {
		app.isTauri = true;
		calls.length = 0;
		answer = { name: "Fix the login bug 2026-11-08.md", folder: "/Users/me/Notes" };
		mockIPC((command, args) => {
			if (command !== "save_export") return null;
			calls.push(args ?? {});
			return answer instanceof Error
				? Promise.reject({ kind: "failed", message: answer.message })
				: answer;
		});
	});

	afterEach(() => {
		clearMocks();
		delete app.isTauri;
		localStorage.clear();
	});

	it("suggests the title and the day, then opens where the last file went", async () => {
		const asking = vi.fn();
		await expect(
			saveWhenReady(Promise.resolve("# Chat\n"), "Fix the login bug", asking)
		).resolves.toEqual({ step: "saved", name: "Fix the login bug 2026-11-08.md" });
		expect(asking).toHaveBeenCalledTimes(1);
		expect(calls[0]).toEqual({
			text: "# Chat\n",
			name: expect.stringMatching(/^Fix the login bug \d{4}-\d{2}-\d{2}\.md$/),
			folder: null,
		});
		expect(localStorage.getItem(EXPORT_FOLDER_KEY)).toBe("/Users/me/Notes");

		await saveWhenReady(Promise.resolve("# Chat\n"), "Fix the login bug");
		expect(calls[1]).toMatchObject({ folder: "/Users/me/Notes" });
	});

	it("keeps the last folder when you cancel", async () => {
		localStorage.setItem(EXPORT_FOLDER_KEY, "/Users/me/Notes");
		answer = null;
		await expect(saveWhenReady(Promise.resolve("text"), "Chat")).resolves.toEqual({
			step: "cancelled",
		});
		expect(localStorage.getItem(EXPORT_FOLDER_KEY)).toBe("/Users/me/Notes");
	});

	it("gives the reason the conversation didn't load, before any dialog", async () => {
		const asking = vi.fn();
		await expect(
			saveWhenReady(Promise.reject(new Error("devbox isn't connected.")), "Chat", asking)
		).resolves.toEqual({ step: "failed", reason: "devbox isn't connected." });
		expect(asking).not.toHaveBeenCalled();
		expect(calls).toEqual([]);
	});

	it("says why the file didn't save", async () => {
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		answer = new Error("You can't save files in Applications.");
		await expect(saveWhenReady(Promise.resolve("text"), "Chat")).resolves.toEqual({
			step: "failed",
			reason: "You can't save files in Applications.",
		});
		expect(localStorage.getItem(EXPORT_FOLDER_KEY)).toBeNull();
		warn.mockRestore();
	});
});
