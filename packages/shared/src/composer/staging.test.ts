// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AgentId, AttachmentRef, DeviceId } from "../bindings";
import type { ActionResult } from "../data/source";

import {
	allReady,
	clipboardFileOf,
	createStaging,
	kindOfMime,
	MAX_PHONE_FILE,
	type PickedFile,
	readyId,
	settled,
	tooLarge,
	tooLargeText,
} from "./staging";

const DEVICE = "device-1" as DeviceId;
const AGENT = "agent-1" as AgentId;

const ref = (id: string, name: string, kind: AttachmentRef["kind"] = "file"): AttachmentRef => ({
	id,
	name,
	kind,
	bytes: 3,
	mime: null,
	path: `/staged/${name}`,
	thumbnail: null,
});

const staged = (attachment: AttachmentRef): ActionResult =>
	({ status: "ok", data: { status: "staged", attachment } }) as ActionResult;

const refused = (message: string): ActionResult => ({
	status: "error",
	error: { kind: "failed", message },
});

const fakeSource = (results: { pasted?: ActionResult; found?: ActionResult } = {}) => {
	const calls = { pasted: [] as unknown[][], found: [] as unknown[][] };
	return {
		calls,
		source: {
			stagePasted: async (...args: unknown[]) => {
				calls.pasted.push(args);
				return results.pasted ?? staged(ref("p1", String(args[1]), "image"));
			},
			stageFound: async (...args: unknown[]) => {
				calls.found.push(args);
				return [results.found ?? staged(ref("f1", "notes.md"))];
			},
		} as never,
	};
};

let counter = 0;
const keys = () => `k${(counter += 1)}`;

const picked = (size: number | null, bytes = new Uint8Array([1, 2, 3])): PickedFile => ({
	name: "photo.jpg",
	mime: "image/jpeg",
	size,
	preview: "file:///photo.jpg",
	read: async () => bytes,
});

describe("tooLarge", () => {
	it("refuses a file over 8 MiB and lets one at the limit through", () => {
		expect(tooLarge(MAX_PHONE_FILE)).toBe(false);
		expect(tooLarge(MAX_PHONE_FILE + 1)).toBe(true);
		expect(tooLarge(null)).toBe(false);
	});

	it("names the limit in megabytes", () => {
		expect(tooLargeText()).toBe("Too large to send from the phone yet (up to 8 MB)");
	});
});

describe("kindOfMime", () => {
	it("guesses pictures, PDFs and other files", () => {
		expect(kindOfMime("image/heic")).toBe("image");
		expect(kindOfMime("application/pdf")).toBe("pdf");
		expect(kindOfMime("text/plain")).toBe("file");
	});
});

describe("createStaging", () => {
	it("stages a picked photo as its bytes and marks it ready", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, keys);
		const result = staging.getState().addPicked(picked(3));
		expect("key" in result).toBe(true);
		expect(staging.getState().files[0]?.state).toBe("staging");
		const files = await settled(staging);
		expect(files[0]).toMatchObject({ state: "ready", kind: "image", ref: { id: "p1" } });
		expect(calls.pasted[0]?.slice(0, 3)).toEqual([DEVICE, "photo.jpg", "image/jpeg"]);
		expect(allReady(files)).toBe(true);
	});

	it("keeps a file the picker says is too big out of the box", () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, keys);
		const result = staging.getState().addPicked(picked(MAX_PHONE_FILE + 1));
		expect(result).toEqual({ refused: tooLargeText() });
		expect(staging.getState().files).toEqual([]);
		expect(calls.pasted).toEqual([]);
	});

	it("fails a file whose bytes turn out too big without sending them", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, keys);
		staging.getState().addPicked(picked(null, new Uint8Array(MAX_PHONE_FILE + 1)));
		const files = await settled(staging);
		expect(files[0]).toMatchObject({ state: "failed", error: tooLargeText() });
		expect(calls.pasted).toEqual([]);
	});

	it("shows why maxd refused a file", async () => {
		const { source } = fakeSource({ pasted: refused("Disk is full") });
		const staging = createStaging(source, DEVICE, keys);
		staging.getState().addPicked(picked(3));
		const files = await settled(staging);
		expect(files[0]).toMatchObject({ state: "failed", error: "Disk is full" });
		expect(allReady(files)).toBe(false);
	});

	it("stages a file the menu found from the agent's folder", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, keys);
		const key = staging.getState().addFound(AGENT, DEVICE, "docs/notes.md");
		expect(staging.getState().files[0]).toMatchObject({ name: "notes.md", kind: "file" });
		const files = await settled(staging);
		expect(calls.found[0]).toEqual([AGENT, ["docs/notes.md"]]);
		expect(readyId(files, key)).toBe("f1");
	});

	it("names a found folder by its last part", () => {
		const { source } = fakeSource();
		const staging = createStaging(source, DEVICE, keys);
		staging.getState().addFound(AGENT, DEVICE, "src/pills/");
		expect(staging.getState().files[0]).toMatchObject({ name: "pills", kind: "folder" });
	});

	it("brings back a saved draft's files, staging a found one again", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, keys);
		staging.getState().restore([
			{
				key: "a",
				name: "a.png",
				kind: "image",
				source: null,
				ref: ref("r1", "a.png"),
				device: DEVICE,
			},
			{
				key: "b",
				name: "notes.md",
				kind: "file",
				source: null,
				ref: null,
				device: null,
				found: { agent: AGENT, device: DEVICE, path: "notes.md" },
			},
			{ key: "c", name: "gone.txt", kind: "file", source: null, ref: null, device: null },
		]);
		const files = await settled(staging);
		expect(files.map((file) => [file.key, file.state])).toEqual([
			["a", "ready"],
			["b", "ready"],
			["c", "failed"],
		]);
		expect(calls.found).toHaveLength(1);
	});

	it("drops a file taken out while it stages", async () => {
		const { source } = fakeSource();
		const staging = createStaging(source, DEVICE, keys);
		const result = staging.getState().addPicked(picked(3));
		if (!("key" in result)) throw new Error("refused");
		staging.getState().remove(result.key);
		expect(await settled(staging)).toEqual([]);
	});

	it("keeps only the files the text still holds", () => {
		const { source } = fakeSource();
		const staging = createStaging(source, DEVICE, keys);
		const one = staging.getState().addReady(ref("r1", "one.txt"));
		staging.getState().addReady(ref("r2", "two.txt"));
		staging.getState().keepOnly(new Set([one]));
		expect(staging.getState().files.map((file) => file.key)).toEqual([one]);
	});
});

describe("clipboardFileOf", () => {
	it("keeps the staged copy with its device", async () => {
		const { source } = fakeSource();
		const staging = createStaging(source, DEVICE, keys);
		staging.getState().addReady(ref("r1", "one.txt"));
		const [file] = await settled(staging);
		if (!file) throw new Error("no file");
		expect(clipboardFileOf(file, DEVICE)).toMatchObject({
			name: "one.txt",
			ref: { id: "r1" },
			device: DEVICE,
			found: null,
		});
	});
});
