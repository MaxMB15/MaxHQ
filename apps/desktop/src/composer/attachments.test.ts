// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { beforeAll, describe, expect, it } from "vitest";

import type { ActionResult } from "@maxhq/shared/data/source";

import {
	chipDetail,
	chipCount,
	chipSummary,
	chipType,
	createAttachments,
	formatBytes,
	stagedIds,
} from "./attachments";
import {
	AGENT,
	attachment,
	DEVICE,
	deferred,
	fakeNative,
	fakeSource,
	heldAgent,
	installBrowserFakes,
	staged,
} from "./testing";

const setup = () => {
	const { source } = fakeSource({ agent: heldAgent() });
	const { native } = fakeNative();
	const store = createAttachments();
	return { store, source, native, context: { source, device: DEVICE, native } };
};

const states = (store: ReturnType<typeof createAttachments>) =>
	store.getState().chips.map((chip) => [chip.name, chip.state]);

beforeAll(installBrowserFakes);

describe("attachments", () => {
	it("shows a chip at once, fills in its details, then stages it", async () => {
		const { store, source, native, context } = setup();
		const staging = deferred<ActionResult[]>();
		source.stageAttachments.mockReturnValueOnce(staging.promise);
		native.describePaths.mockResolvedValueOnce([
			{
				path: "/Users/max/main.rs",
				name: "main.rs",
				kind: "file",
				size: 4100,
				mime: "text/x-rust",
			},
		]);

		store.getState().addPaths(context, ["/Users/max/main.rs"]);
		expect(states(store)).toEqual([["main.rs", "staging"]]);
		expect(chipDetail(store.getState().chips[0]!)).toBe("Getting it ready");
		expect(source.stageAttachments).toHaveBeenCalledWith(DEVICE, ["/Users/max/main.rs"]);

		await Promise.resolve();
		expect(store.getState().chips[0]!.bytes).toBe(4100);

		staging.resolve([staged(attachment("main.rs", { bytes: 4100 }))]);
		await store.getState().settled();
		expect(states(store)).toEqual([["main.rs", "staged"]]);
		expect(chipDetail(store.getState().chips[0]!)).toBe("Rust, 4 KB");
		expect(stagedIds(store.getState().chips)).toEqual(["att-main.rs"]);
	});

	it("marks a chip failed with the reason maxd gave", async () => {
		const { store, source, context } = setup();
		source.stageAttachments.mockResolvedValueOnce([
			{ status: "ok", data: { status: "refused", reason: "Too big to send" } },
		]);

		store.getState().addPaths(context, ["/Users/max/huge.mov"]);
		await store.getState().settled();

		const [chip] = store.getState().chips;
		expect(chip!.state).toBe("failed");
		expect(chipDetail(chip!)).toBe("Too big to send");
		expect(stagedIds(store.getState().chips)).toEqual([]);
	});

	it("stages what the @ menu found on the agent's device, without uploading", async () => {
		const { store, source, native, context } = setup();
		source.stageFound.mockResolvedValueOnce([
			staged(attachment("main.rs")),
			{
				status: "ok",
				data: { status: "refused", reason: "gone.rs: nothing is at that path" },
			},
		]);

		const remote = { ...context, target: { type: "device" } as const };
		store.getState().addPaths(remote, ["src/main.rs", "gone.rs"], { foundIn: AGENT });
		await store.getState().settled();

		expect(source.stageFound).toHaveBeenCalledWith(AGENT, ["src/main.rs", "gone.rs"]);
		expect(source.stageAttachments).not.toHaveBeenCalled();
		expect(source.upload).not.toHaveBeenCalled();
		expect(native.describePaths).not.toHaveBeenCalled();
		expect(states(store)).toEqual([
			["main.rs", "staged"],
			["gone.rs", "failed"],
		]);
		expect(store.getState().chips.map((chip) => chip.source)).toEqual([null, null]);
	});

	it("marks a chip failed when staging only says done", async () => {
		const { store, source, context } = setup();
		source.stageAttachments.mockResolvedValueOnce([{ status: "ok", data: { status: "done" } }]);

		store.getState().addPaths(context, ["/Users/max/notes.txt"]);
		await store.getState().settled();

		expect(chipDetail(store.getState().chips[0]!)).toBe("Couldn't get it ready");
	});

	it("stages pasted bytes, with a preview for an image", async () => {
		const { store, source, context } = setup();

		store.getState().addBytes(context, "pasted-image.png", "image/png", new Uint8Array(3));
		const [chip] = store.getState().chips;
		expect(chip!.kind).toBe("image");
		expect(chip!.preview).toMatch(/^blob:/);

		await store.getState().settled();
		expect(source.stagePasted).toHaveBeenCalledWith(
			DEVICE,
			"pasted-image.png",
			"image/png",
			new Uint8Array(3)
		);
		expect(states(store)).toEqual([["pasted-image.png", "staged"]]);
	});

	it("waits for every chip in flight to settle", async () => {
		const { store, source, context } = setup();
		const first = deferred<ActionResult[]>();
		const second = deferred<ActionResult>();
		source.stageAttachments.mockReturnValueOnce(first.promise);
		source.stagePasted.mockReturnValueOnce(second.promise);
		store.getState().addPaths(context, ["/a.txt"]);
		store.getState().addBytes(context, "b.txt", "text/plain", new Uint8Array(1));

		let settled = false;
		void store
			.getState()
			.settled()
			.then(() => {
				settled = true;
			});
		first.resolve([staged(attachment("a.txt"))]);
		await Promise.resolve();
		await Promise.resolve();
		expect(settled).toBe(false);

		second.resolve(staged(attachment("b.txt")));
		await store.getState().settled();
		expect(settled).toBe(true);
	});

	it("restores staged attachments and removes chips", () => {
		const { store } = setup();
		store.getState().restore([attachment("a.rs"), attachment("b.png", { kind: "image" })]);
		expect(states(store)).toEqual([
			["a.rs", "staged"],
			["b.png", "staged"],
		]);

		store.getState().remove(store.getState().chips[0]!.key);
		expect(states(store)).toEqual([["b.png", "staged"]]);
		store.getState().clear();
		expect(store.getState().chips).toEqual([]);
	});
});

describe("chip text", () => {
	it("sizes like the mockup", () => {
		expect(formatBytes(900)).toBe("900 B");
		expect(formatBytes(4100)).toBe("4 KB");
		expect(formatBytes(1.3 * 1024 * 1024)).toBe("1.3 MB");
	});

	it("names the kind of file", () => {
		expect(chipType({ name: "main.rs", kind: "file", mime: null })).toBe("Rust");
		expect(chipType({ name: "shot.jpeg", kind: "image", mime: "image/jpeg" })).toBe("JPG");
		expect(chipType({ name: "spec.pdf", kind: "pdf", mime: null })).toBe("PDF");
		expect(chipType({ name: "src", kind: "folder", mime: null })).toBe("Folder");
		expect(chipType({ name: "Makefile", kind: "file", mime: null })).toBe("File");
	});

	it("sums the chips for the footer", () => {
		const { store } = setup();
		store
			.getState()
			.restore([
				attachment("a.rs", { bytes: 1024 * 1024 }),
				attachment("b.rs", { bytes: 300 * 1024 }),
			]);
		expect(chipSummary(store.getState().chips)).toBe("2 attachments, 1.3 MB");
		expect(chipCount(store.getState().chips)).toBe("2 attachments");
		expect(chipCount(store.getState().chips.slice(1))).toBe("1 attachment");
	});
});
