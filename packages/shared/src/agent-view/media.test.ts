// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import type { DataSource, MediaResult, TransferProgress } from "../data/source";
import {
	cancelMedia,
	clearMedia,
	FULL_LIMIT,
	loadMedia,
	mediaFailed,
	mediaLost,
	mediaState,
	PICTURE_LIMIT,
	PICTURES_AT_ONCE,
} from "./media";
import { CODE_LIMIT } from "./viewer-files";

afterEach(() => clearMedia());

const AGENT = "agent-1";
const PATH = "shots/frame.png";
const URL = "maxmedia://localhost/frame.png";

// A source whose fetches wait until the test says how they go.
const fetches = () => {
	const calls: {
		limit: number;
		progress: (progress: TransferProgress) => void;
		finish: (result: MediaResult) => void;
	}[] = [];
	const media = vi.fn<DataSource["media"]>(
		(_agent, _path, limit, _anyway, onProgress) =>
			new Promise<MediaResult>((resolve) => {
				calls.push({ limit, progress: onProgress, finish: resolve });
			})
	);
	const cancelTransfer = vi.fn<DataSource["cancelTransfer"]>(async () => {});
	const source = { media, cancelTransfer } as unknown as DataSource;
	const last = () => {
		const call = calls.at(-1);
		if (!call) throw new Error("nothing is fetching");
		return call;
	};
	return { source, media, cancelTransfer, calls, last };
};

const cached = (size: number): MediaResult => ({
	status: "ok",
	data: { kind: "cached", url: URL, size },
});

const large = (size: number): MediaResult => ({
	status: "ok",
	data: { kind: "too_large", size },
});

const progress = (sent: number, total: number): TransferProgress => ({
	transfer: "t-1",
	sent,
	total,
	file: "frame.png",
	state: "uploading",
});

describe("media", () => {
	it("fetches a picture once, however many rows ask", async () => {
		const { source, media, last } = fetches();
		const first = loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		void loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		expect(media).toHaveBeenCalledTimes(1);
		expect(mediaState(AGENT, PATH)).toEqual({ state: "loading", sent: 0, total: 0 });

		last().progress(progress(10, 40));
		expect(mediaState(AGENT, PATH)).toEqual({ state: "loading", sent: 10, total: 40 });

		last().finish(cached(40));
		await first;
		expect(mediaState(AGENT, PATH)).toEqual({ state: "ready", url: URL, size: 40 });

		// Once it's here, asking again changes nothing.
		await loadMedia(source, AGENT, PATH, FULL_LIMIT);
		expect(media).toHaveBeenCalledTimes(1);
	});

	it("waits with a large one until it's asked for with room for it", async () => {
		const { source, media, last } = fetches();
		const sight = loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		last().finish(large(64 * 1024 * 1024));
		await sight;
		expect(mediaState(AGENT, PATH)).toEqual({ state: "large", size: 64 * 1024 * 1024 });

		// Scrolling back into view asks with the picture limit again.
		await loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		expect(media).toHaveBeenCalledTimes(1);

		const click = loadMedia(source, AGENT, PATH, FULL_LIMIT);
		expect(media).toHaveBeenCalledTimes(2);
		expect(last().limit).toBe(FULL_LIMIT);
		// It knows the size while it waits for the first progress.
		expect(mediaState(AGENT, PATH)).toEqual({
			state: "loading",
			sent: 0,
			total: 64 * 1024 * 1024,
		});
		last().finish(cached(64 * 1024 * 1024));
		await click;
		expect(mediaState(AGENT, PATH)).toMatchObject({ state: "ready" });
	});

	it("cancels a fetch and waits for a click again", async () => {
		const { source, cancelTransfer, last } = fetches();
		const click = loadMedia(source, AGENT, PATH, FULL_LIMIT);
		last().progress(progress(10, 400));
		cancelMedia(source, AGENT, PATH);
		expect(cancelTransfer).toHaveBeenCalledWith("t-1");
		expect(mediaState(AGENT, PATH)).toEqual({ state: "large", size: 400 });

		// The result still coming in changes nothing.
		last().finish({ status: "error", error: { kind: "failed", message: "Cancelled." } });
		await click;
		expect(mediaState(AGENT, PATH)).toEqual({ state: "large", size: 400 });
	});

	it("cancels a fetch the device hasn't named yet as soon as it does", async () => {
		const { source, cancelTransfer, last } = fetches();
		void loadMedia(source, AGENT, PATH, FULL_LIMIT);
		cancelMedia(source, AGENT, PATH);
		expect(cancelTransfer).not.toHaveBeenCalled();
		expect(mediaState(AGENT, PATH)).toBeNull();

		last().progress(progress(0, 400));
		expect(cancelTransfer).toHaveBeenCalledWith("t-1");
		expect(mediaState(AGENT, PATH)).toBeNull();
	});

	it("says why a fetch failed, and tries again when asked", async () => {
		const { source, media, last } = fetches();
		const first = loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		last().finish({ status: "error", error: { kind: "not_connected", device: "Studio" } });
		await first;
		expect(mediaState(AGENT, PATH)).toEqual({
			state: "failed",
			reason: "Studio isn't connected.",
		});

		const again = loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		expect(media).toHaveBeenCalledTimes(2);
		last().finish(cached(40));
		await again;
		expect(mediaState(AGENT, PATH)).toMatchObject({ state: "ready" });
	});

	it("fetches again what the webview couldn't show", async () => {
		const { source, media, last } = fetches();
		const first = loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		last().finish(cached(40));
		await first;
		mediaFailed(AGENT, PATH, "Couldn't show frame.png.");
		expect(mediaState(AGENT, PATH)).toEqual({
			state: "failed",
			reason: "Couldn't show frame.png.",
		});
		void loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		expect(media).toHaveBeenCalledTimes(2);
	});

	it("fetches once more a picture the cache let go of, then fails", async () => {
		const { source, media, last } = fetches();
		const first = loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		last().finish(cached(40));
		await first;

		const again = mediaLost(source, AGENT, PATH, "Couldn't show frame.png.");
		expect(media).toHaveBeenCalledTimes(2);
		expect(last().limit).toBe(PICTURE_LIMIT);
		expect(mediaState(AGENT, PATH)).toEqual({ state: "loading", sent: 0, total: 0 });
		// Another row that couldn't show it waits for the same fetch.
		await mediaLost(source, AGENT, PATH, "Couldn't show frame.png.");
		expect(media).toHaveBeenCalledTimes(2);
		last().finish(cached(40));
		await again;
		expect(mediaState(AGENT, PATH)).toEqual({ state: "ready", url: URL, size: 40 });

		await mediaLost(source, AGENT, PATH, "Couldn't show frame.png.");
		expect(media).toHaveBeenCalledTimes(2);
		expect(mediaState(AGENT, PATH)).toEqual({
			state: "failed",
			reason: "Couldn't show frame.png.",
		});
	});

	it("fetches a lost picture you clicked for with room for it", async () => {
		const { source, last } = fetches();
		const first = loadMedia(source, AGENT, PATH, FULL_LIMIT);
		last().finish(cached(PICTURE_LIMIT + 1));
		await first;

		void mediaLost(source, AGENT, PATH, "Couldn't show frame.png.");
		expect(last().limit).toBe(FULL_LIMIT);
	});

	it("fetches a few pictures on sight at once, and the rest in turn", async () => {
		const { source, media, calls } = fetches();
		const paths = Array.from({ length: PICTURES_AT_ONCE + 2 }, (_, i) => `shots/${i}.png`);
		const loads = paths.map((path) => loadMedia(source, AGENT, path, PICTURE_LIMIT));
		expect(media).toHaveBeenCalledTimes(PICTURES_AT_ONCE);
		// The ones waiting show as loading too.
		expect(mediaState(AGENT, paths.at(-1)!)).toEqual({ state: "loading", sent: 0, total: 0 });

		calls[0]!.finish(cached(40));
		await vi.waitFor(() => expect(media).toHaveBeenCalledTimes(PICTURES_AT_ONCE + 1));
		expect(media).toHaveBeenLastCalledWith(
			AGENT,
			paths[PICTURES_AT_ONCE],
			PICTURE_LIMIT,
			false,
			expect.any(Function)
		);

		// A failed one makes room as well.
		calls[1]!.finish({ status: "error", error: { kind: "failed", message: "Gone." } });
		await vi.waitFor(() => expect(media).toHaveBeenCalledTimes(PICTURES_AT_ONCE + 2));
		for (const call of calls.slice(2)) call.finish(cached(40));
		await Promise.all(loads);
		expect(mediaState(AGENT, paths.at(-1)!)).toMatchObject({ state: "ready" });
	});

	it("fetches a clicked picture now, whatever is waiting", async () => {
		const { source, media, last } = fetches();
		for (let i = 0; i < PICTURES_AT_ONCE; i++) {
			void loadMedia(source, AGENT, `shots/${i}.png`, PICTURE_LIMIT);
		}
		void loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		expect(media).toHaveBeenCalledTimes(PICTURES_AT_ONCE);

		// One waiting its turn goes as soon as it's clicked, with room for all of it.
		void loadMedia(source, AGENT, PATH, FULL_LIMIT);
		await vi.waitFor(() => expect(media).toHaveBeenCalledTimes(PICTURES_AT_ONCE + 1));
		expect(last().limit).toBe(FULL_LIMIT);

		// One nobody had asked for doesn't wait either.
		void loadMedia(source, AGENT, "shots/big.png", FULL_LIMIT);
		expect(media).toHaveBeenCalledTimes(PICTURES_AT_ONCE + 2);
	});

	it("never fetches a picture cancelled while it waits", async () => {
		const { source, media, calls } = fetches();
		for (let i = 0; i < PICTURES_AT_ONCE; i++) {
			void loadMedia(source, AGENT, `shots/${i}.png`, PICTURE_LIMIT);
		}
		const waits = loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		void loadMedia(source, AGENT, "shots/next.png", PICTURE_LIMIT);
		cancelMedia(source, AGENT, PATH);
		await waits;
		expect(mediaState(AGENT, PATH)).toBeNull();

		// Its turn goes to the next one.
		calls[0]!.finish(cached(40));
		await vi.waitFor(() => expect(media).toHaveBeenCalledTimes(PICTURES_AT_ONCE + 1));
		expect(media).toHaveBeenLastCalledWith(
			AGENT,
			"shots/next.png",
			PICTURE_LIMIT,
			false,
			expect.any(Function)
		);
		expect(media).not.toHaveBeenCalledWith(
			AGENT,
			PATH,
			expect.anything(),
			expect.anything(),
			expect.anything()
		);
	});

	it("fetches a file pane's file and Show anyway now, whatever is waiting", async () => {
		const { source, media, last } = fetches();
		for (let i = 0; i < PICTURES_AT_ONCE; i++) {
			void loadMedia(source, AGENT, `shots/${i}.png`, PICTURE_LIMIT);
		}
		const outside = "/tmp/example/notes.ts";
		const opened = loadMedia(source, AGENT, outside, CODE_LIMIT);
		expect(media).toHaveBeenCalledTimes(PICTURES_AT_ONCE + 1);
		last().finish({
			status: "ok",
			data: { kind: "refused", outside: true, message: "outside the agent's folders" },
		});
		await opened;
		// The refusal still lets the pane offer Show anyway.
		expect(mediaState(AGENT, outside)).toMatchObject({ refused: "outside" });

		void loadMedia(source, AGENT, outside, CODE_LIMIT, true);
		expect(media).toHaveBeenCalledTimes(PICTURES_AT_ONCE + 2);
		expect(media).toHaveBeenLastCalledWith(
			AGENT,
			outside,
			CODE_LIMIT,
			true,
			expect.any(Function)
		);
		void loadMedia(source, AGENT, "/tmp/example/shot.png", PICTURE_LIMIT, true);
		expect(media).toHaveBeenCalledTimes(PICTURES_AT_ONCE + 3);
	});

	it("keeps each agent's files apart", async () => {
		const { source, media } = fetches();
		void loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		void loadMedia(source, "agent-2", PATH, PICTURE_LIMIT);
		expect(media).toHaveBeenCalledTimes(2);
		expect(mediaState("agent-2", PATH)).toMatchObject({ state: "loading" });
	});

	it("says a path is outside the agent's folders, and keeps Show anyway's fetch apart", async () => {
		const { source, media, last } = fetches();
		const outside = "/tmp/example/plan.md";
		const first = loadMedia(source, AGENT, outside, PICTURE_LIMIT);
		expect(media.mock.calls[0]?.[3]).toBe(false);
		last().finish({
			status: "ok",
			data: {
				kind: "refused",
				outside: true,
				message: "the path is outside the agent's folders",
			},
		});
		await first;
		expect(mediaState(AGENT, outside)).toEqual({
			state: "failed",
			reason: "The path is outside the agent's folders.",
			refused: "outside",
		});

		const shown = loadMedia(source, AGENT, outside, PICTURE_LIMIT, true);
		expect(media.mock.calls[1]?.[3]).toBe(true);
		last().finish(cached(40));
		await shown;
		expect(mediaState(AGENT, outside, true)).toMatchObject({ state: "ready" });
		// A fetch on sight still finds the refusal, not the file.
		expect(mediaState(AGENT, outside)).toMatchObject({ refused: "outside" });
	});

	it("says a bad path was refused, apart from one outside", async () => {
		const { source, last } = fetches();
		const first = loadMedia(source, AGENT, PATH, PICTURE_LIMIT);
		last().finish({
			status: "ok",
			data: { kind: "refused", outside: false, message: "no such file" },
		});
		await first;
		expect(mediaState(AGENT, PATH)).toMatchObject({ refused: "path", reason: "No such file." });
	});
});
