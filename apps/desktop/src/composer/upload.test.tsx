// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, describe, expect, it } from "vitest";

import type { ActionResult, TransferProgress } from "@maxhq/shared/data/source";
import type { UploadTarget } from "@maxhq/shared/bindings";
import { OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";

import { type Chip, chipDetail, createAttachments, stagedIds } from "./attachments";
import { MAX_ENTRIES, refusalFor, TOO_LARGE, TOO_MANY } from "./limits";
import {
	AGENT,
	attachment,
	deferred,
	DEVICE,
	editorOf,
	fakeNative,
	fakeSource,
	findTextbox,
	heldAgent,
	installBrowserFakes,
	pillsIn,
	REMOTE,
	remoteLink,
	renderComposer,
	staged,
	textIn,
	type,
} from "./testing";
import { largeWarning, rateOf, uploadTotals } from "./upload";

const MB = 1024 * 1024;
const GB = 1024 * MB;
const TARGET: UploadTarget = { type: "agent", agent: AGENT };

/** An upload a test drives by hand: its progress callback and its answer. */
const held = (transfer = "t-1") => {
	const answer = deferred<ActionResult[]>();
	let report: ((progress: TransferProgress) => void) | null = null;
	return {
		answer,
		/** Stands in for the source's upload. */
		upload: async (
			_device: string,
			_target: UploadTarget,
			_paths: string[],
			onProgress: (progress: TransferProgress) => void
		) => {
			report = onProgress;
			return answer.promise;
		},
		progress: (sent: number, total: number, state: TransferProgress["state"] = "uploading") => {
			if (!report) throw new Error("The upload hasn't started");
			report({ transfer, sent, total, file: "main.rs", state });
		},
	};
};

const setup = () => {
	const { source } = fakeSource({ agent: heldAgent(), links: [remoteLink()] });
	const native = fakeNative();
	const store = createAttachments();
	const context = { source, device: REMOTE, native: native.native, target: TARGET };
	return { store, source, native: native.native, context };
};

const only = (store: ReturnType<typeof createAttachments>): Chip => {
	const [chip] = store.getState().chips;
	if (!chip) throw new Error("No chip");
	return chip;
};

/** Lets the describe call and the upload's start go through. */
const started = () => act(async () => {});

beforeAll(installBrowserFakes);

describe("uploads", () => {
	it("uploads a file to a remote device, shows its progress, then says it copied", async () => {
		const { store, source, native, context } = setup();
		const upload = held();
		source.upload.mockImplementationOnce(upload.upload);
		native.describePaths.mockResolvedValueOnce([
			{
				path: "/Users/max/main.rs",
				name: "main.rs",
				kind: "file",
				size: 1.2 * MB,
				mime: null,
			},
		]);

		store.getState().addPaths(context, ["/Users/max/main.rs"]);
		expect(only(store).state).toBe("uploading");
		expect(source.stageAttachments).not.toHaveBeenCalled();
		await started();
		expect(source.upload).toHaveBeenCalledWith(
			REMOTE,
			TARGET,
			["/Users/max/main.rs"],
			expect.any(Function)
		);

		act(() => upload.progress(0.8 * MB, 1.2 * MB));
		expect(chipDetail(only(store))).toBe("0.8 of 1.2 MB");
		expect(uploadTotals(store.getState().chips).percent).toBe(66);
		act(() => upload.progress(0.8 * MB, 1.2 * MB, "paused"));
		expect(chipDetail(only(store))).toBe("Paused");

		upload.answer.resolve([staged(attachment("main.rs", { bytes: 18 * 1024 }))]);
		await store.getState().settled();
		expect(only(store).state).toBe("staged");
		expect(chipDetail(only(store))).toBe("Copied, 18 KB");
		expect(stagedIds(store.getState().chips)).toEqual(["att-main.rs"]);
	});

	it("uploads pasted bytes", async () => {
		const { store, source, context } = setup();
		store.getState().addBytes(context, "notes.txt", "text/plain", new Uint8Array(5));
		await store.getState().settled();
		expect(source.uploadPasted).toHaveBeenCalledWith(
			REMOTE,
			TARGET,
			"notes.txt",
			"text/plain",
			new Uint8Array(5),
			expect.any(Function)
		);
		expect(chipDetail(only(store))).toBe("Copied, 5 B");
	});

	it("stops the transfer when its chip goes, even before the transfer has an ID", async () => {
		const { store, source, context } = setup();
		const first = held();
		source.upload.mockImplementationOnce(first.upload);
		store.getState().addPaths(context, ["/Users/max/a.rs"]);
		await started();
		act(() => first.progress(10, 100));
		store.getState().remove(only(store).key);
		expect(source.cancelTransfer).toHaveBeenCalledWith("t-1");

		const second = held("t-2");
		source.upload.mockImplementationOnce(second.upload);
		store.getState().addPaths(context, ["/Users/max/b.rs"]);
		await started();
		store.getState().remove(only(store).key);
		second.progress(10, 100);
		expect(source.cancelTransfer).toHaveBeenLastCalledWith("t-2");
		second.answer.resolve([staged(attachment("b.rs"))]);
		await act(async () => {});
		expect(store.getState().chips).toEqual([]);
	});

	it("offers Retry when an upload fails, and runs it again", async () => {
		const { store, source, context } = setup();
		source.upload.mockResolvedValueOnce([
			{ status: "error", error: { kind: "failed", message: "Link dropped" } },
		]);
		store.getState().addPaths(context, ["/Users/max/a.rs"]);
		await store.getState().settled();
		expect(only(store)).toMatchObject({ state: "failed", retryable: true });
		expect(uploadTotals(store.getState().chips).retryable).toBe(1);

		store.getState().retry(only(store).key);
		expect(only(store).state).toBe("uploading");
		await store.getState().settled();
		expect(only(store).state).toBe("staged");
		expect(source.upload).toHaveBeenCalledTimes(2);
	});

	it("cancels every upload at once", async () => {
		const { store, source, context } = setup();
		const upload = held();
		source.upload.mockImplementation(upload.upload);
		store.getState().restore([attachment("kept.rs")]);
		store.getState().addPaths(context, ["/Users/max/a.rs", "/Users/max/b.rs"]);
		await started();
		store.getState().cancelUploads();
		expect(store.getState().chips.map((chip) => chip.name)).toEqual(["kept.rs"]);
	});
});

describe("files from a timeline", () => {
	const SHOT = {
		agent: "a2",
		path: "/attachments/a2/shot.png",
		name: "shot.png",
		kind: "image",
		bytes: 1.2 * MB,
		mime: "image/png",
	} as const;

	it("names a file from an agent by its path, not by what the timeline calls it", () => {
		const { store, source, native } = setup();
		store
			.getState()
			.addFromAgent(
				{ source, device: DEVICE, native },
				{ ...SHOT, path: "/home/you/.env", name: "holiday.png" }
			);
		expect(only(store)).toMatchObject({ name: ".env" });
	});

	it("copies a file from the agent that has it, then stages it here", async () => {
		const { store, source, native } = setup();
		const answer = deferred<ActionResult>();
		let report: ((progress: TransferProgress) => void) | null = null;
		source.attachFromAgent.mockImplementationOnce(async (...args) => {
			report = args[4];
			return answer.promise;
		});

		store.getState().addFromAgent({ source, device: DEVICE, native }, SHOT);
		expect(only(store)).toMatchObject({ name: "shot.png", kind: "image", state: "uploading" });
		expect(source.attachFromAgent).toHaveBeenCalledWith(
			DEVICE,
			null,
			"a2",
			"/attachments/a2/shot.png",
			expect.any(Function)
		);
		act(() =>
			report?.({
				transfer: "t-1",
				sent: 0.6 * MB,
				total: 1.2 * MB,
				file: "shot.png",
				state: "uploading",
			})
		);
		expect(chipDetail(only(store))).toBe("0.6 of 1.2 MB");

		answer.resolve(staged(attachment("shot.png", { bytes: 1.2 * MB })));
		await store.getState().settled();
		expect(stagedIds(store.getState().chips)).toEqual(["att-shot.png"]);
	});

	it("sends it on to an agent on another device", async () => {
		const { store, source, context } = setup();
		store.getState().addFromAgent(context, SHOT);
		await store.getState().settled();
		expect(source.attachFromAgent).toHaveBeenCalledWith(
			REMOTE,
			TARGET,
			"a2",
			"/attachments/a2/shot.png",
			expect.any(Function)
		);
		expect(only(store).state).toBe("staged");
	});

	it("adds the same file again as a second chip", async () => {
		const { store, source, context } = setup();
		const first = store.getState().addFromAgent(context, SHOT);
		const second = store.getState().addFromAgent(context, SHOT);
		await store.getState().settled();
		expect(first).not.toBe(second);
		expect(store.getState().chips.map((chip) => chip.name)).toEqual(["shot.png", "shot.png"]);
		expect(source.attachFromAgent).toHaveBeenCalledTimes(2);
	});

	it("offers Retry when the copy fails, and runs it again", async () => {
		const { store, source, context } = setup();
		source.attachFromAgent.mockRejectedValueOnce(new Error("Studio isn't connected."));
		store.getState().addFromAgent(context, SHOT);
		await store.getState().settled();
		expect(only(store)).toMatchObject({ state: "failed", retryable: true });

		store.getState().retry(only(store).key);
		await store.getState().settled();
		expect(only(store).state).toBe("staged");
		expect(source.attachFromAgent).toHaveBeenCalledTimes(2);
	});

	it("says why the agent's device wouldn't give it", async () => {
		const { store, source, context } = setup();
		source.attachFromAgent.mockResolvedValueOnce({
			status: "ok",
			data: { status: "refused", reason: "That file is outside the agent's folder." },
		});
		store.getState().addFromAgent(context, SHOT);
		await store.getState().settled();
		expect(only(store)).toMatchObject({ state: "failed", retryable: false });
		// A chip's detail drops the full stop.
		expect(chipDetail(only(store))).toBe("That file is outside the agent's folder");
	});
});

describe("limits", () => {
	it("turns away a file over 2 GB before it copies", async () => {
		const { store, source, native, context } = setup();
		native.describePaths.mockResolvedValueOnce([
			{
				path: "/Users/max/disk.img",
				name: "disk.img",
				kind: "file",
				size: 3 * GB,
				mime: null,
			},
		]);
		store.getState().addPaths(context, ["/Users/max/disk.img"]);
		await store.getState().settled();
		expect(only(store)).toMatchObject({ state: "failed", error: TOO_LARGE, retryable: false });
		expect(source.upload).not.toHaveBeenCalled();
	});

	it("counts every file in the message toward the 2 GB", async () => {
		const { store, source, native, context } = setup();
		native.describePaths.mockResolvedValueOnce([
			{ path: "/a.mov", name: "a.mov", kind: "file", size: 1.5 * GB, mime: null },
			{ path: "/b.mov", name: "b.mov", kind: "file", size: 1.5 * GB, mime: null },
		]);
		store.getState().addPaths(context, ["/a.mov", "/b.mov"]);
		await store.getState().settled();
		expect(store.getState().chips.map((chip) => chip.state)).toEqual(["staged", "failed"]);
		expect(source.upload).toHaveBeenCalledTimes(1);
	});

	it("stops a folder whose upload turns out over 2 GB", async () => {
		const { store, source, context } = setup();
		const upload = held();
		source.upload.mockImplementationOnce(upload.upload);
		store.getState().addPaths(context, ["/Users/max/photos/"]);
		await started();
		act(() => upload.progress(0, 2.5 * GB));
		expect(only(store)).toMatchObject({ state: "failed", error: TOO_LARGE });
		expect(source.cancelTransfer).toHaveBeenCalledWith("t-1");
	});

	it("turns away the entry past 20,000", () => {
		const chips = Array.from({ length: MAX_ENTRIES + 1 }, (_, index): Chip => ({
			key: `chip-${index}`,
			name: `${index}.txt`,
			kind: "file",
			bytes: 1,
			mime: null,
			source: null,
			preview: null,
			state: "uploading",
			ref: null,
			error: null,
			upload: null,
			retryable: false,
		}));
		expect(refusalFor(chips, `chip-${MAX_ENTRIES - 1}`)).toBeNull();
		expect(refusalFor(chips, `chip-${MAX_ENTRIES}`)).toBe(TOO_MANY);
	});

	it("warns about a file of 100 MB or more", async () => {
		const { store, native, context, source } = setup();
		source.upload.mockImplementationOnce(held().upload);
		native.describePaths.mockResolvedValueOnce([
			{ path: "/a.mov", name: "a.mov", kind: "file", size: 140 * MB, mime: null },
		]);
		store.getState().addPaths(context, ["/a.mov"]);
		await started();
		expect(largeWarning(store.getState().chips)).toBe(
			"a.mov is 140.0 MB, so it takes a while to copy."
		);
	});

	it("tells the speed from the samples", () => {
		expect(rateOf([{ at: 0, sent: 0 }])).toBeNull();
		expect(
			rateOf([
				{ at: 0, sent: 0 },
				{ at: 1000, sent: 2 * MB },
			])
		).toBe(2 * MB);
	});
});

describe("the upload strip", () => {
	const remote = () => {
		const native = fakeNative();
		const rendered = renderComposer({ native, links: [remoteLink()], on: REMOTE });
		return rendered;
	};

	const strip = () => screen.getByRole("group", { name: "Upload" });

	it("shows the total, and Send waits for the upload", async () => {
		const { source, drop } = remote();
		const upload = held();
		source.upload.mockImplementationOnce(upload.upload);
		const textbox = await findTextbox();

		act(() => drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
		await started();
		expect(strip()).toHaveTextContent("Uploading 1 file to devbox… 0%");
		act(() => upload.progress(512 * 1024, 1024 * 1024));
		expect(strip()).toHaveTextContent("50%");
		expect(strip()).toHaveTextContent("0.5 of 1.0 MB");

		await type(textbox, "Look at this");
		fireEvent.click(screen.getByRole("button", { name: /^Send/ }));
		expect(await screen.findByText("Sends when the upload finishes")).toBeInTheDocument();
		expect(source.sendInput).not.toHaveBeenCalled();

		await act(async () => upload.answer.resolve([staged(attachment("main.rs"))]));
		await waitFor(() =>
			expect(source.sendInput).toHaveBeenCalledWith(
				AGENT,
				{
					text: `${MARK} Look at this`,
					attachments: ["att-main.rs"],
					refs: [{ type: "attachment", id: "att-main.rs" }],
				},
				"queue"
			)
		);
		expect(screen.queryByRole("group", { name: "Upload" })).toBeNull();
	});

	it("says it's paused while the link is down", async () => {
		const { source, drop, upsert } = remote();
		source.upload.mockImplementationOnce(held().upload);
		await findTextbox();
		act(() => drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
		await started();

		upsert([
			{
				entity: "device_link",
				...remoteLink({
					state: { state: "unreachable", reason: "Tailscale is off", last_seen: null },
				}),
			},
		]);
		expect(strip()).toHaveTextContent("Paused, reconnecting to devbox");
	});

	it("cancels the upload and the send that waits for it, and keeps the words", async () => {
		const { source, drop } = remote();
		source.upload.mockImplementationOnce(held().upload);
		const textbox = await findTextbox();
		act(() => drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
		await started();
		await type(textbox, "Look at this");
		fireEvent.click(screen.getByRole("button", { name: /^Send/ }));
		await screen.findByText("Sends when the upload finishes");

		fireEvent.click(within(strip()).getByRole("button", { name: "Cancel upload" }));
		await waitFor(() => expect(pillsIn(textbox)).toEqual([]));
		expect(source.sendInput).not.toHaveBeenCalled();
		expect(textIn(textbox)).toBe("Look at this");
	});

	it("retries failed uploads from the strip", async () => {
		const { source, drop } = remote();
		source.upload.mockResolvedValueOnce([
			{ status: "error", error: { kind: "failed", message: "Link dropped" } },
		]);
		const textbox = await findTextbox();
		act(() => drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
		expect(await screen.findByText("Couldn't upload 1 file to devbox")).toBeInTheDocument();
		expect(pillsIn(textbox)).toEqual([expect.stringContaining("failed, Link dropped")]);

		fireEvent.click(within(strip()).getByRole("button", { name: "Retry" }));
		await waitFor(() => expect(source.upload).toHaveBeenCalledTimes(2));
		await waitFor(() => expect(screen.queryByRole("group", { name: "Upload" })).toBeNull());
	});

	it("retries or removes a failed file from its pill's preview", async () => {
		const { source, drop } = remote();
		source.upload.mockResolvedValue([
			{ status: "error", error: { kind: "failed", message: "Link dropped" } },
		]);
		const textbox = await findTextbox();
		act(() => drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
		await waitFor(() =>
			expect(pillsIn(textbox)).toEqual([expect.stringContaining("failed, Link dropped")])
		);

		// Selecting the pill opens its preview, which has what to do about it.
		act(() => {
			editorOf(textbox).commands.setNodeSelection(1);
		});
		const preview = await screen.findByRole("dialog");
		expect(preview).toHaveTextContent("Link dropped");
		fireEvent.click(within(preview).getByRole("button", { name: "Retry" }));
		await waitFor(() => expect(source.upload).toHaveBeenCalledTimes(2));

		await waitFor(() =>
			expect(
				within(screen.getByRole("dialog")).getByRole("button", { name: "Remove" })
			).toBeEnabled()
		);
		fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Remove" }));
		await waitFor(() => expect(pillsIn(textbox)).toEqual([]));
		expect(screen.queryByRole("group", { name: "Upload" })).toBeNull();
	});

	it("refuses to send past the limit", async () => {
		const native = fakeNative();
		native.native.describePaths.mockResolvedValueOnce([
			{ path: "/disk.img", name: "disk.img", kind: "file", size: 3 * GB, mime: null },
		]);
		const { source, drop } = renderComposer({ native, links: [remoteLink()], on: REMOTE });
		const textbox = await findTextbox();
		act(() => drop().onDrop(["/disk.img"], { x: 0, y: 0 }));
		await started();
		await type(textbox, "Here");
		fireEvent.keyDown(textbox, { key: "Enter" });
		expect(await screen.findByRole("alert")).toHaveTextContent(
			`Couldn't attach disk.img. ${TOO_LARGE}. Remove it, then send again.`
		);
		expect(source.sendInput).not.toHaveBeenCalled();
	});

	it("stages on this Mac without a strip when the agent runs here", async () => {
		const { source, drop } = renderComposer();
		await findTextbox();
		act(() => drop().onDrop(["/Users/max/main.rs"], { x: 0, y: 0 }));
		await waitFor(() => expect(source.stageAttachments).toHaveBeenCalled());
		expect(source.upload).not.toHaveBeenCalled();
		expect(screen.queryByRole("group", { name: "Upload" })).toBeNull();
	});
});
