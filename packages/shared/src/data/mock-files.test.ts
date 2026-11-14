// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { BINARY_SAMPLE, sampleFile } from "./mock-files";

const bytesOf = async (path: string): Promise<Uint8Array> => {
	const file = sampleFile(path);
	if (!file) throw new Error(`no sample for ${path}`);
	const bytes = new Uint8Array(await (await fetch(file.url)).arrayBuffer());
	expect(bytes.byteLength).toBe(file.size);
	return bytes;
};

describe("the sample files", () => {
	it("has a Markdown plan and a table as text", async () => {
		const plan = new TextDecoder().decode(await bytesOf("docs/plan.md"));
		expect(plan).toMatch(/^# Usage popover/);
		const table = new TextDecoder().decode(await bytesOf("data/usage.CSV"));
		expect(table.split("\n")[0]).toBe("window,account,used,resets");
	});

	it("has a PDF whose cross-reference table points at its objects", async () => {
		const text = new TextDecoder().decode(await bytesOf("mockup.pdf"));
		expect(text.startsWith("%PDF-1.4")).toBe(true);
		const xref = Number(/startxref\n(\d+)/.exec(text)![1]);
		expect(text.slice(xref, xref + 4)).toBe("xref");
		const second = Number(text.slice(xref).split("\n")[3]!.slice(0, 10));
		expect(text.slice(second)).toMatch(/^1 0 obj/);
	});

	it("has a binary glTF with a JSON chunk and a buffer chunk", async () => {
		const bytes = await bytesOf("part.glb");
		const view = new DataView(bytes.buffer);
		expect(view.getUint32(0, true)).toBe(0x46546c67);
		expect(view.getUint32(8, true)).toBe(bytes.byteLength);
		const length = view.getUint32(12, true);
		const json = JSON.parse(new TextDecoder().decode(bytes.subarray(20, 20 + length)));
		expect(view.getUint32(20 + length, true)).toBe(json.buffers[0].byteLength);
		expect(json.accessors[2].count % 3).toBe(0);
	});

	it("has a picture, and nothing for a file the viewer doesn't show", async () => {
		const png = await bytesOf("shot.png");
		expect([...png.subarray(1, 4)]).toEqual([0x50, 0x4e, 0x47]);
		expect(sampleFile("src/Makefile")).toBeNull();
		expect(sampleFile("notes.txt")).toBeNull();
	});

	it("has Rust and TypeScript sources, and a log with a zero byte", async () => {
		const rust = new TextDecoder().decode(await bytesOf("src/main.rs"));
		expect(rust).toMatch(/^\/\/! Usage windows/);
		const ts = new TextDecoder().decode(await bytesOf("src/rows.ts"));
		expect(ts).toContain("export const usageRows");
		expect([...(await bytesOf(BINARY_SAMPLE))]).toContain(0);
	});
});
