// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { setLanguageChoice } from "@maxhq/shared/i18n";
import type { InvokeArgs } from "@tauri-apps/api/core";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { NativeFailure } from "./failure";
import {
	describePaths,
	moveToTrash,
	pickAttachments,
	readClipboardFiles,
	readClipboardImage,
	revealPath,
	thumbnail,
	useThumbnail,
} from "./files";

const app = globalThis as { isTauri?: boolean };
const PNG = [0x89, 0x50, 0x4e, 0x47];

describe("in the app", () => {
	const calls: [string, InvokeArgs | undefined][] = [];

	beforeEach(() => {
		app.isTauri = true;
		calls.length = 0;
		mockIPC((command, args) => {
			calls.push([command, args]);
			switch (command) {
				case "pick_attachments":
					return ["/Users/me/notes", "/Users/me/a.png"];
				case "read_clipboard_files":
					return [];
				case "read_clipboard_image":
					return new Uint8Array(PNG).buffer;
				case "describe_paths":
					return [
						{
							path: "/a.png",
							name: "a.png",
							kind: "image",
							size: 4,
							mime: "image/png",
						},
					];
				case "trash_offered":
					if (args && "path" in args && args.path === "/Users/me/notes") {
						return Promise.reject({
							kind: "failed",
							message: "/Users/me/notes isn't a file",
						});
					}
					return null;
				case "reveal_download":
					if (args && "path" in args && args.path === "/Users/me/notes") {
						return Promise.reject({
							kind: "failed",
							message: "/Users/me/notes isn't a download",
						});
					}
					return null;
				case "thumbnail":
					if (args && "path" in args && args.path === "/gone.png")
						return new ArrayBuffer(0);
					if (args && "path" in args && args.path === "/secret") {
						return Promise.reject({ kind: "not_offered", path: "/secret" });
					}
					return PNG;
			}
		});
	});

	afterEach(() => {
		delete app.isTauri;
		clearMocks();
	});

	it("passes the picker's and clipboard's paths through", async () => {
		await expect(pickAttachments()).resolves.toEqual(["/Users/me/notes", "/Users/me/a.png"]);
		await expect(readClipboardFiles()).resolves.toEqual([]);
	});

	it("passes localized interface labels to the attachment picker", async () => {
		setLanguageChoice("de");
		try {
			await pickAttachments();
			expect(calls.at(-1)).toEqual([
				"pick_attachments",
				{
					labels: {
						prompt: "Anhängen",
						message: "Wähle Dateien oder Ordner zum Anhängen aus.",
					},
				},
			]);
		} finally {
			setLanguageChoice("en");
		}
	});

	it("reads the clipboard image as bytes", async () => {
		const png = await readClipboardImage();
		expect(Array.from(png ?? [])).toEqual(PNG);
	});

	it("describes paths through the backend", async () => {
		const [info] = await describePaths(["/a.png"]);
		expect(info?.kind).toBe("image");
		expect(calls).toEqual([["describe_paths", { paths: ["/a.png"] }]]);
	});

	it("asks for a thumbnail at the screen's pixel ratio", async () => {
		vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(2);
		const png = await thumbnail("/a.png", 48);
		expect(png?.type).toBe("image/png");
		expect(png?.size).toBe(PNG.length);
		expect(calls).toEqual([["thumbnail", { path: "/a.png", size: 48, scale: 2 }]]);
		vi.restoreAllMocks();
	});

	it("gives null for an empty thumbnail and a failure for a refused one", async () => {
		await expect(thumbnail("/gone.png", 48)).resolves.toBeNull();
		const refused = await thumbnail("/secret", 48).catch((error: unknown) => error);
		expect(refused).toBeInstanceOf(NativeFailure);
		expect((refused as NativeFailure).reason).toEqual({ kind: "not_offered", path: "/secret" });
		expect((refused as NativeFailure).message).toBe("/secret wasn't dropped, pasted or picked");
	});

	it("moves a file to the Trash and refuses a folder", async () => {
		await expect(moveToTrash("/Users/me/app.private-key.pem")).resolves.toBeUndefined();
		expect(calls).toEqual([["trash_offered", { path: "/Users/me/app.private-key.pem" }]]);
		const refused = await moveToTrash("/Users/me/notes").catch((error: unknown) => error);
		expect(refused).toBeInstanceOf(NativeFailure);
		expect((refused as NativeFailure).message).toBe("/Users/me/notes isn't a file");
	});

	it("shows a download in Finder and refuses anything else", async () => {
		const landed = "/Users/me/Downloads/MaxHQ/notes.txt";
		await expect(revealPath(landed)).resolves.toBeUndefined();
		expect(calls).toEqual([["reveal_download", { path: landed }]]);
		const refused = await revealPath("/Users/me/notes").catch((error: unknown) => error);
		expect(refused).toBeInstanceOf(NativeFailure);
		expect((refused as NativeFailure).message).toBe("/Users/me/notes isn't a download");
	});

	it("keeps a thumbnail URL while it's shown and revokes it after", async () => {
		const created = vi.fn((blob: Blob) => `blob:${blob.size}`);
		const revoked = vi.fn();
		URL.createObjectURL = created;
		URL.revokeObjectURL = revoked;

		const { result, rerender, unmount } = renderHook(({ path }) => useThumbnail(path, 48), {
			initialProps: { path: "/a.png" as string | null },
		});
		expect(result.current).toBeNull();
		await waitFor(() => expect(result.current).toBe("blob:4"));

		rerender({ path: "/gone.png" });
		expect(result.current).toBeNull();
		expect(revoked).toHaveBeenCalledWith("blob:4");
		await waitFor(() => expect(calls.at(-1)?.[1]).toMatchObject({ path: "/gone.png" }));
		expect(result.current).toBeNull();

		rerender({ path: "/a.png" });
		await waitFor(() => expect(result.current).toBe("blob:4"));
		unmount();
		expect(revoked).toHaveBeenCalledTimes(2);
	});
});

describe("in a browser", () => {
	it("has nothing to pick, paste or preview", async () => {
		await expect(pickAttachments()).resolves.toEqual([]);
		await expect(readClipboardFiles()).resolves.toEqual([]);
		await expect(readClipboardImage()).resolves.toBeNull();
		await expect(thumbnail("/a.png", 48)).resolves.toBeNull();
		await expect(moveToTrash("/a.pem")).resolves.toBeUndefined();
		await expect(revealPath("/a.txt")).resolves.toBeUndefined();
	});

	it("guesses descriptions from the names", async () => {
		const infos = await describePaths([
			"/a/photo.JPG",
			"/a/spec.pdf",
			"/a/src/",
			"/a/notes.txt",
		]);
		expect(infos.map((info) => [info.name, info.kind])).toEqual([
			["photo.JPG", "image"],
			["spec.pdf", "pdf"],
			["src", "folder"],
			["notes.txt", "file"],
		]);
	});
});
