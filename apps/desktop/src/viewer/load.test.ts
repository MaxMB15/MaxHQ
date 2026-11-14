// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { mockConvertFileSrc } from "@tauri-apps/api/mocks";
import { describe, expect, it } from "vitest";

import { readBytes, viewerFrameUrl } from "./load";

// A cache that answers like the app's: whole up to `whole` bytes, and in
// chunks of `chunk` past that.
const cache = (file: Uint8Array, whole: number, chunk: number) => {
	const asked: (string | null)[] = [];
	const get = (async (_url: string, init?: RequestInit) => {
		const range = (init?.headers as Record<string, string> | undefined)?.Range ?? null;
		asked.push(range);
		const start = range ? Number(/bytes=(\d+)-/.exec(range)![1]) : 0;
		if (!range && file.byteLength <= whole) return new Response(file.slice());
		const end = Math.min(start + chunk, file.byteLength);
		return new Response(file.slice(start, end), {
			status: 206,
			headers: { "Content-Range": `bytes ${start}-${end - 1}/${file.byteLength}` },
		});
	}) as typeof fetch;
	return { get, asked };
};

const file = Uint8Array.from({ length: 1000 }, (_, at) => at % 251);

describe("reading a cached file", () => {
	it("takes a small file in one answer", async () => {
		const { get, asked } = cache(file, 2000, 300);
		expect(new Uint8Array(await readBytes("u", 1000, get))).toEqual(file);
		expect(asked).toEqual([null]);
	});

	it("asks for each next range of a large one", async () => {
		const { get, asked } = cache(file, 100, 300);
		expect(new Uint8Array(await readBytes("u", 1000, get))).toEqual(file);
		expect(asked).toEqual([null, "bytes=300-", "bytes=600-", "bytes=900-"]);
	});

	it("fails on an answer that isn't the file", async () => {
		const get = (async () => new Response(null, { status: 404 })) as typeof fetch;
		await expect(readBytes("u", 10, get)).rejects.toThrow("HTTP 404");
	});
});

describe("the viewer frame's page", () => {
	it("comes from the app's scheme in the built app, and beside the page otherwise", () => {
		expect(viewerFrameUrl(true, true)).toBe("/viewer.html");
		expect(viewerFrameUrl(false, false)).toBe("/viewer.html");
		mockConvertFileSrc("macos");
		expect(viewerFrameUrl(false, true)).toBe("maxview://localhost/viewer.html");
		mockConvertFileSrc("windows");
		expect(viewerFrameUrl(false, true)).toBe("http://maxview.localhost/viewer.html");
	});
});
