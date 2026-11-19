// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { AgentId, AttachmentRef, DeviceId, TransferId } from "../bindings";
import type { ActionResult, TransferProgress } from "../data/source";
import { EMPTY_VIEW } from "../view/replica";
import { MAX_BYTES, TOO_LARGE } from "./limits";

import {
	allReady,
	clipboardFileOf,
	createStaging,
	kindOfMime,
	MAX_PHONE_FILE,
	type PickedFile,
	readyId,
	settled,
	takesFiles,
	tooLarge,
	tooLargeText,
	uploadPercent,
	uploadText,
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

/** An upload the test moves along by hand. */
type Going = {
	readonly args: unknown[];
	readonly progress: (sent: number, total: number) => void;
	readonly finish: (result: ActionResult) => void;
};

const fakeSource = (results: { pasted?: ActionResult; found?: ActionResult } = {}) => {
	const calls = {
		pasted: [] as unknown[][],
		found: [] as unknown[][],
		uploads: [] as Going[],
		cancelled: [] as TransferId[],
	};
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
			upload: (...args: unknown[]) =>
				new Promise<ActionResult[]>((resolve) => {
					const onProgress = args[3] as (progress: TransferProgress) => void;
					const transfer = `t${calls.uploads.length + 1}` as TransferId;
					calls.uploads.push({
						args,
						progress: (sent, total) =>
							onProgress({
								transfer,
								sent,
								total,
								file: "photo.jpg",
								state: "uploading",
							}),
						finish: (result) => resolve([result]),
					});
				}),
			cancelTransfer: async (transfer: TransferId) => {
				calls.cancelled.push(transfer);
			},
		} as never,
	};
};

let counter = 0;
const keys = () => `k${(counter += 1)}`;

const picked = (
	size: number | null,
	bytes = new Uint8Array([1, 2, 3]),
	path: string | null = "/caches/photo.jpg"
): PickedFile => ({
	name: "photo.jpg",
	mime: "image/jpeg",
	size,
	preview: "file:///photo.jpg",
	path,
	read: async () => bytes,
});

/** Lets the store hear what the fake source just did. */
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

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
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const result = staging.getState().addPicked(picked(3), false);
		expect("key" in result).toBe(true);
		expect(staging.getState().files[0]?.state).toBe("staging");
		const files = await settled(staging);
		expect(files[0]).toMatchObject({ state: "ready", kind: "image", ref: { id: "p1" } });
		expect(calls.pasted[0]?.slice(0, 3)).toEqual([DEVICE, "photo.jpg", "image/jpeg"]);
		expect(allReady(files)).toBe(true);
	});

	it("keeps a file the picker says is too big out of the box", () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const result = staging.getState().addPicked(picked(MAX_PHONE_FILE + 1), false);
		expect(result).toEqual({ refused: tooLargeText() });
		expect(staging.getState().files).toEqual([]);
		expect(calls.pasted).toEqual([]);
	});

	it("fails a file whose bytes turn out too big without sending them", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		staging.getState().addPicked(picked(null, new Uint8Array(MAX_PHONE_FILE + 1)), false);
		const files = await settled(staging);
		expect(files[0]).toMatchObject({ state: "failed", error: tooLargeText() });
		expect(calls.pasted).toEqual([]);
	});

	it("shows why maxd refused a file", async () => {
		const { source } = fakeSource({ pasted: refused("Disk is full") });
		const staging = createStaging(source, DEVICE, AGENT, keys);
		staging.getState().addPicked(picked(3), false);
		const files = await settled(staging);
		expect(files[0]).toMatchObject({ state: "failed", error: "Disk is full" });
		expect(allReady(files)).toBe(false);
	});

	it("stages a file the menu found from the agent's folder", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const key = staging.getState().addFound(AGENT, DEVICE, "docs/notes.md");
		expect(staging.getState().files[0]).toMatchObject({ name: "notes.md", kind: "file" });
		const files = await settled(staging);
		expect(calls.found[0]).toEqual([AGENT, ["docs/notes.md"]]);
		expect(readyId(files, key)).toBe("f1");
	});

	it("names a found folder by its last part", () => {
		const { source } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		staging.getState().addFound(AGENT, DEVICE, "src/pills/");
		expect(staging.getState().files[0]).toMatchObject({ name: "pills", kind: "folder" });
	});

	it("brings back a saved draft's files, staging a found one again", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
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
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const result = staging.getState().addPicked(picked(3), false);
		if (!("key" in result)) throw new Error("refused");
		staging.getState().remove(result.key);
		expect(await settled(staging)).toEqual([]);
	});

	it("keeps only the files the text still holds", () => {
		const { source } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const one = staging.getState().addReady(ref("r1", "one.txt"));
		staging.getState().addReady(ref("r2", "two.txt"));
		staging.getState().keepOnly(new Set([one]));
		expect(staging.getState().files.map((file) => file.key)).toEqual([one]);
	});
});

describe("createStaging, to a device that takes uploads", () => {
	it("uploads a picked file to the agent and shows how far it has got", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const result = staging.getState().addPicked(picked(MAX_PHONE_FILE * 4), true);
		if (!("key" in result)) throw new Error("refused");
		expect(calls.pasted).toEqual([]);
		const going = calls.uploads[0];
		if (!going) throw new Error("no upload");
		expect(going.args.slice(0, 3)).toEqual([
			DEVICE,
			{ type: "agent", agent: AGENT },
			["/caches/photo.jpg"],
		]);
		const first = () => staging.getState().files[0];
		expect(uploadPercent(first()!)).toBe(0);
		going.progress(MAX_PHONE_FILE, MAX_PHONE_FILE * 4);
		expect(uploadPercent(first()!)).toBe(25);
		expect(uploadText(first()!)).toBe("25% sent");
		going.finish(staged(ref("u1", "photo.jpg", "image")));
		const files = await settled(staging);
		expect(files[0]).toMatchObject({ state: "ready", ref: { id: "u1" } });
		expect(uploadPercent(files[0]!)).toBeNull();
		expect(uploadText(files[0]!)).toBeNull();
	});

	it("says when an upload waits for room or for the link", () => {
		const file = {
			key: "k",
			name: "photo.jpg",
			kind: "image",
			bytes: 10,
			preview: null,
			state: "staging",
			ref: null,
			error: null,
			found: null,
			upload: { transfer: null, sent: 0, total: 10, state: "paused" },
		} as const;
		expect(uploadText(file)).toBe("Paused while the link is down");
		expect(uploadText({ ...file, upload: { ...file.upload, state: "waiting" } })).toBe(
			"Waiting for room on the device"
		);
	});

	it("sends the bytes when the device doesn't take uploads or the file has no path", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		staging.getState().addPicked(picked(3), false);
		staging.getState().addPicked(picked(3, undefined, null), true);
		await settled(staging);
		expect(calls.uploads).toEqual([]);
		expect(calls.pasted).toHaveLength(2);
	});

	it("turns away a file that would take the message over 2 GB", () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		staging.getState().addPicked(picked(MAX_BYTES - 10), true);
		const result = staging.getState().addPicked(picked(20), true);
		expect(result).toEqual({ refused: TOO_LARGE });
		expect(calls.uploads).toHaveLength(1);
	});

	it("stops an upload that turns out larger than a message carries", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		staging.getState().addPicked(picked(null), true);
		calls.uploads[0]?.progress(0, MAX_BYTES + 1);
		expect(staging.getState().files[0]).toMatchObject({ state: "failed", error: TOO_LARGE });
		await tick();
		expect(calls.cancelled).toEqual(["t1"]);
	});

	it("stops the upload of a file taken out of the box", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const one = staging.getState().addPicked(picked(100), true);
		const two = staging.getState().addPicked(picked(100), true);
		if (!("key" in one) || !("key" in two)) throw new Error("refused");
		calls.uploads[0]?.progress(10, 100);
		staging.getState().remove(one.key);
		// The second hasn't said its transfer yet, so it stops on its first progress.
		staging.getState().keepOnly(new Set());
		calls.uploads[1]?.progress(10, 100);
		calls.uploads[1]?.finish(staged(ref("u2", "photo.jpg")));
		await tick();
		expect(calls.cancelled).toEqual(["t1", "t2"]);
		expect(staging.getState().files).toEqual([]);
	});
});

describe("createStaging, with the picker's copies", () => {
	/** A picked file whose copy counts how often it's deleted. */
	const copied = (size: number | null, path: string | null = "/caches/photo.jpg") => {
		const deleted = { count: 0 };
		const file: PickedFile = {
			...picked(size, undefined, path),
			discard: () => deleted.count++,
		};
		return { file, deleted };
	};

	it("deletes the copy once the upload succeeds, and drops the picture it drew", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const { file, deleted } = copied(100);
		staging.getState().addPicked(file, true);
		calls.uploads[0]?.progress(50, 100);
		expect(deleted.count).toBe(0);
		calls.uploads[0]?.finish(staged(ref("u1", "photo.jpg", "image")));
		const [ready] = await settled(staging);
		expect(ready).toMatchObject({ state: "ready", preview: null });
		expect(deleted.count).toBe(1);
		staging.getState().clear();
		expect(deleted.count).toBe(1);
	});

	it("deletes the copy once its bytes are staged", async () => {
		const { source } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const { file, deleted } = copied(3);
		staging.getState().addPicked(file, false);
		await settled(staging);
		expect(deleted.count).toBe(1);
	});

	it("keeps the picture of a file the app didn't copy", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		staging.getState().addPicked(picked(100), true);
		calls.uploads[0]?.finish(staged(ref("u1", "photo.jpg", "image")));
		const [ready] = await settled(staging);
		expect(ready?.preview).toBe("file:///photo.jpg");
	});

	it("keeps a failed file's copy until it leaves the box", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const { file, deleted } = copied(100);
		const result = staging.getState().addPicked(file, true);
		if (!("key" in result)) throw new Error("refused");
		calls.uploads[0]?.finish(refused("disk full"));
		await settled(staging);
		expect(deleted.count).toBe(0);
		staging.getState().remove(result.key);
		expect(deleted.count).toBe(1);
	});

	it("deletes the copy of a file taken out, or dropped from the text, while it uploads", async () => {
		const { source, calls } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const one = copied(100);
		const two = copied(100);
		const three = copied(100);
		const first = staging.getState().addPicked(one.file, true);
		const second = staging.getState().addPicked(two.file, true);
		staging.getState().addPicked(three.file, true);
		if (!("key" in first) || !("key" in second)) throw new Error("refused");
		staging.getState().remove(first.key);
		expect([one.deleted.count, two.deleted.count, three.deleted.count]).toEqual([1, 0, 0]);
		staging.getState().keepOnly(new Set([second.key]));
		expect([one.deleted.count, two.deleted.count, three.deleted.count]).toEqual([1, 0, 1]);
		staging.getState().clear();
		expect([one.deleted.count, two.deleted.count, three.deleted.count]).toEqual([1, 1, 1]);
		// An upload that ends after its file left doesn't delete it again.
		for (const going of calls.uploads) going.finish(staged(ref("u", "photo.jpg")));
		await tick();
		expect([one.deleted.count, two.deleted.count, three.deleted.count]).toEqual([1, 1, 1]);
	});

	it("deletes the copy of a file the box turns away", () => {
		const { source } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
		const big = copied(MAX_PHONE_FILE + 1);
		expect(staging.getState().addPicked(big.file, false)).toEqual({
			refused: tooLargeText(),
		});
		expect(big.deleted.count).toBe(1);
		staging.getState().addPicked(picked(MAX_BYTES - 10), true);
		const over = copied(20);
		expect(staging.getState().addPicked(over.file, true)).toEqual({ refused: TOO_LARGE });
		expect(over.deleted.count).toBe(1);
	});
});

describe("clipboardFileOf", () => {
	it("keeps the staged copy with its device", async () => {
		const { source } = fakeSource();
		const staging = createStaging(source, DEVICE, AGENT, keys);
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

describe("takesFiles", () => {
	it("is false for a device with no link", () => {
		expect(takesFiles(EMPTY_VIEW, DEVICE)).toBe(false);
	});
});
