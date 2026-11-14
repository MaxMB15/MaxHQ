// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import type { DataSource, MediaResult, TransferProgress } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import type { AttachmentRef, ItemBody, TimelineItem } from "@maxhq/shared/bindings";
import { NOW } from "@maxhq/shared/test/build";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";
import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import { clearMedia, FULL_LIMIT, PICTURE_LIMIT } from "@maxhq/shared/agent-view/media";

import { clearDownloads } from "./download";
import { ItemBodyView } from "./item-row";

afterEach(() => {
	clearMedia();
	clearDownloads();
	clearKept();
});

const MB = 1024 * 1024;
const cachedUrl = (name: string) => `maxmedia://localhost/${name}`;

// A source whose fetches wait until the test says how they go.
const fetches = () => {
	const calls: {
		path: string;
		limit: number;
		progress: (progress: TransferProgress) => void;
		finish: (result: MediaResult) => void;
	}[] = [];
	const media = vi.fn<DataSource["media"]>(
		(_agent, path, limit, _anyway, onProgress) =>
			new Promise<MediaResult>((resolve) => {
				calls.push({ path, limit, progress: onProgress, finish: resolve });
			})
	);
	const cancelTransfer = vi.fn<DataSource["cancelTransfer"]>(async () => {});
	const source: DataSource = {
		...mockSource({ now: () => NOW, tick: false }),
		media,
		cancelTransfer,
	};
	const last = () => {
		const call = calls.at(-1);
		if (!call) throw new Error("nothing is fetching");
		return call;
	};
	const finish = (result: MediaResult, call = last()) =>
		act(async () => {
			call.finish(result);
		});
	return {
		source,
		media,
		cancelTransfer,
		last,
		finish,
		cached: (name: string, size = 2048, call = last()) =>
			finish({ status: "ok", data: { kind: "cached", url: cachedUrl(name), size } }, call),
		large: (size: number, call = last()) =>
			finish({ status: "ok", data: { kind: "too_large", size } }, call),
		progress: (sent: number, total: number) =>
			act(() =>
				last().progress({
					transfer: "t-1",
					sent,
					total,
					file: "clip.mp4",
					state: "uploading",
				})
			),
	};
};

const show = (
	item: TimelineItem,
	{ remote = false, source }: { remote?: boolean; source: DataSource }
) => {
	const value: RowContext = {
		agent: "a1",
		vendor: "claude",
		timeline: createTimelineStore(),
		readOnly: false,
		remote,
		answer: async () => ({ status: "ok", data: { status: "done" } }),
		subAgentsShown: false,
		toggleSubAgents: () => {},
	};
	const wrap = (children: ReactNode) => (
		<ServicesProvider source={source}>
			<FixedNow value={NOW}>
				<RowProvider value={value}>{children}</RowProvider>
			</FixedNow>
		</ServicesProvider>
	);
	return render(wrap(<ItemBodyView item={item} />));
};

const item = (body: ItemBody) =>
	timelineItem(body, {
		id: `item-${body.kind}`,
		status: "completed",
		started_at: NOW - 4000,
		completed_at: NOW,
	});

const reply = (text: string) => item({ kind: "assistant_message", text, streaming: false });

const picture = (name: string, thumbnail: string | null = null): AttachmentRef => ({
	id: `att-${name}`,
	name,
	kind: "image",
	bytes: 48_213,
	mime: "image/png",
	path: `/attachments/a1/${name}`,
	thumbnail,
});

const PREVIEW = "data:image/jpeg;base64,/9j/4AAQSkZJRg==";

describe("pictures in a reply", () => {
	it("fetch from the agent's device and open whole on a click", async () => {
		const transfers = fetches();
		show(reply("Here it is:\n\n![The popover](shots/popover.png)"), {
			source: transfers.source,
		});
		expect(transfers.media).toHaveBeenCalledWith(
			"a1",
			"shots/popover.png",
			PICTURE_LIMIT,
			false,
			expect.any(Function)
		);
		expect(screen.getByRole("status", { name: "Loading The popover" })).toBeInTheDocument();

		await transfers.cached("a.png");
		const shown = screen.getByRole("button", { name: "Show The popover full size" });
		expect(shown).toHaveAttribute("src", cachedUrl("a.png"));
		expect(shown).toHaveAttribute("alt", "The popover");

		fireEvent.click(shown);
		const dialog = await screen.findByRole("dialog");
		expect(dialog.querySelector("img")).toHaveAttribute("src", cachedUrl("a.png"));
		// It's here already, so nothing more is fetched.
		expect(transfers.media).toHaveBeenCalledTimes(1);
	});

	it("wait for a click when they're large, showing the size", async () => {
		const transfers = fetches();
		show(reply("![Full page](/w/shots/page.png)"), { source: transfers.source });
		await transfers.large(24 * MB);
		const button = screen.getByRole("button", { name: "Show Full page, 25 MB" });
		expect(button).toHaveTextContent("Show picture, 25 MB");

		fireEvent.click(button);
		expect(transfers.last().limit).toBe(FULL_LIMIT);
		await transfers.cached("page.png", 24 * MB);
		expect(screen.getByRole("button", { name: "Show Full page full size" })).toHaveAttribute(
			"src",
			cachedUrl("page.png")
		);
	});

	it("say why one didn't come, and try again", async () => {
		const transfers = fetches();
		show(reply("![Chart](out/chart.png)"), { source: transfers.source });
		await transfers.finish({
			status: "error",
			error: { kind: "failed", message: "That path is outside the agent's folder." },
		});
		expect(screen.getByRole("alert")).toHaveTextContent(
			"Image unavailable · Chart · That path is outside the agent's folder."
		);
		fireEvent.click(screen.getByRole("button", { name: "Try loading Chart again" }));
		expect(transfers.media).toHaveBeenCalledTimes(2);
	});

	it("show a picture written into the reply without fetching it", () => {
		const transfers = fetches();
		show(reply(`![Dot](${PREVIEW})`), { source: transfers.source });
		expect(screen.getByRole("button", { name: "Show Dot full size" })).toHaveAttribute(
			"src",
			PREVIEW
		);
		expect(transfers.media).not.toHaveBeenCalled();
	});

	it("link to one on the web instead of loading it", () => {
		const transfers = fetches();
		show(reply("![Diagram](https://example.com/diagram.png)"), { source: transfers.source });
		const link = screen.getByRole("link", { name: "Diagram" });
		expect(link).toHaveAttribute("href", "https://example.com/diagram.png");
		expect(link).toHaveAttribute("target", "_blank");
		expect(document.querySelector("img")).toBeNull();
		expect(transfers.media).not.toHaveBeenCalled();
	});

	it("keep the alt text for anything else", () => {
		const transfers = fetches();
		show(reply("![Notes](notes.txt) and ![](~/Pictures/a.png)"), {
			source: transfers.source,
		});
		expect(screen.getByText("[Notes]")).toBeInTheDocument();
		expect(screen.getByText("[image]")).toBeInTheDocument();
		expect(transfers.media).not.toHaveBeenCalled();
	});
});

describe("videos in a reply", () => {
	it("ask for nothing until a click, then show the size as it comes", async () => {
		const transfers = fetches();
		show(reply("![The flow](demo/flow.mp4#t=2)"), { source: transfers.source });
		// Even its size would make the device read the whole file.
		expect(transfers.media).not.toHaveBeenCalled();
		const play = screen.getByRole("button", { name: "Play The flow" });
		expect(play).toHaveTextContent(/^Load video$/);

		fireEvent.click(play);
		expect(screen.queryByRole("button", { name: "Play The flow" })).toBeNull();
		expect(transfers.last()).toMatchObject({ path: "demo/flow.mp4", limit: FULL_LIMIT });
		const loading = screen.getByRole("status", { name: "Loading The flow" });
		expect(loading).not.toHaveTextContent("%");
		transfers.progress(12 * MB, 48 * MB);
		expect(loading).toHaveTextContent("25% of 50 MB");

		await transfers.cached("flow.mp4", 48 * MB);
		const video = document.querySelector("video");
		expect(video).toHaveAttribute("src", `${cachedUrl("flow.mp4")}#t=2`);
		expect(video).toHaveAttribute("controls");
	});

	it("cancel a fetch and offer it again with its size", async () => {
		const transfers = fetches();
		show(reply("![The flow](demo/flow.mp4)"), { source: transfers.source });
		fireEvent.click(screen.getByRole("button", { name: "Play The flow" }));
		transfers.progress(MB, 48 * MB);
		fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
		expect(transfers.cancelTransfer).toHaveBeenCalledWith("t-1");
		expect(screen.getByRole("button", { name: "Play The flow" })).toHaveTextContent(
			"Load video, 50 MB"
		);
	});

	it("say when one is too large to play here", async () => {
		const transfers = fetches();
		show(reply("![](demo/long.mov)"), { source: transfers.source });
		fireEvent.click(screen.getByRole("button", { name: "Play long.mov" }));
		await transfers.large(2 * FULL_LIMIT);
		expect(screen.getByText(/too large to play here/)).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: /Play/ })).toBeNull();
	});
});

describe("pictures you sent", () => {
	it("open whole on a click, with the preview while it comes", async () => {
		const transfers = fetches();
		show(
			item({
				kind: "user_message",
				text: "Like this",
				attachments: [picture("shot.png", PREVIEW)],
				delivery: "sent",
			}),
			{ remote: true, source: transfers.source }
		);
		// The thumbnail needs no fetch.
		expect(transfers.media).not.toHaveBeenCalled();

		fireEvent.click(screen.getByRole("button", { name: "Show shot.png full size" }));
		const dialog = await screen.findByRole("dialog");
		expect(transfers.last()).toMatchObject({
			path: "/attachments/a1/shot.png",
			limit: FULL_LIMIT,
		});
		expect(dialog.querySelector("img")).toHaveAttribute("src", PREVIEW);
		expect(screen.getByRole("status", { name: "Loading shot.png" })).toBeInTheDocument();

		await transfers.cached("shot.png");
		expect(screen.getByRole("img", { name: "shot.png" })).toHaveAttribute(
			"src",
			cachedUrl("shot.png")
		);
	});

	it("say why one can't open", async () => {
		const transfers = fetches();
		show(
			item({
				kind: "user_message",
				text: "",
				attachments: [picture("shot.png")],
				delivery: "sent",
			}),
			{ source: transfers.source }
		);
		fireEvent.click(screen.getByRole("button", { name: "Show shot.png full size" }));
		await screen.findByRole("dialog");
		await transfers.finish({
			status: "error",
			error: { kind: "not_connected", device: "Studio" },
		});
		expect(screen.getByRole("alert")).toHaveTextContent("Studio isn't connected.");
		fireEvent.click(screen.getByRole("button", { name: "Try again" }));
		expect(transfers.media).toHaveBeenCalledTimes(2);
	});
});

describe("pictures from tools", () => {
	it("show under a tool's row, opening whole on a click", async () => {
		const transfers = fetches();
		show(
			item({
				kind: "tool_call",
				server: "browser",
				name: "screenshot",
				title: null,
				input: null,
				output: "Took a screenshot",
				images: [picture("image-3f9a2c1b.png", PREVIEW)],
			}),
			{ source: transfers.source }
		);
		const thumb = screen.getByRole("button", { name: "Show image-3f9a2c1b.png full size" });
		expect(thumb.querySelector("img")).toHaveAttribute("src", PREVIEW);
		fireEvent.click(thumb);
		await screen.findByRole("dialog");
		expect(transfers.last().path).toBe("/attachments/a1/image-3f9a2c1b.png");
	});

	it("show under a read of a picture", () => {
		show(
			item({
				kind: "file_read",
				path: "shots/frame.png",
				lines: null,
				images: [picture("image-0a1b2c3d.png", PREVIEW)],
			}),
			{ source: fetches().source }
		);
		expect(screen.getByText("shots/frame.png")).toBeInTheDocument();
		expect(
			screen.getByRole("button", { name: "Show image-0a1b2c3d.png full size" })
		).toBeInTheDocument();
	});
});
