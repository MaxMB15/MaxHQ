// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { browserNative, type ComposerNative, ComposerNativeProvider } from "@/composer/native";
import { mockSource } from "@/data/mock-source";
import type { DataSource, DownloadResult, TransferProgress } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import type {
	AttachmentRef,
	FileEdit,
	ItemBody,
	ItemStatus,
	TimelineItem,
} from "@maxhq/shared/bindings";
import { NOW } from "@maxhq/shared/test/build";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { clearDownloads } from "./download";
import { ItemBodyView } from "./item-row";
import { clearKept } from "@maxhq/shared/agent-view/kept";

afterEach(() => {
	clearDownloads();
	clearKept();
});

// A source whose downloads wait until the test says how they go.
const downloads = () => {
	const calls: {
		path: string;
		progress: (progress: TransferProgress) => void;
		finish: (result: DownloadResult) => void;
	}[] = [];
	const download = vi.fn<DataSource["download"]>(
		(_agent, path, onProgress) =>
			new Promise<DownloadResult>((resolve) => {
				calls.push({ path, progress: onProgress, finish: resolve });
			})
	);
	const cancelTransfer = vi.fn<DataSource["cancelTransfer"]>(async () => {});
	const source: DataSource = {
		...mockSource({ now: () => NOW, tick: false }),
		download,
		cancelTransfer,
	};
	const last = () => {
		const call = calls.at(-1);
		if (!call) throw new Error("nothing is downloading");
		return call;
	};
	return {
		source,
		download,
		cancelTransfer,
		progress: (
			sent: number,
			state: TransferProgress["state"] = "uploading",
			transfer = "t-1"
		) => act(() => last().progress({ transfer, sent, total: 1024, file: "app.ts", state })),
		finish: (result: DownloadResult, call = last()) =>
			act(async () => {
				call.finish(result);
			}),
		last,
	};
};

const LANDED = "/Users/max/Downloads/MaxHQ/app.ts";
const OK: DownloadResult = { status: "ok", data: { path: LANDED } };

const show = (
	item: TimelineItem,
	{
		remote = true,
		source = downloads().source,
		native = browserNative,
	}: { remote?: boolean; source?: DataSource; native?: ComposerNative } = {}
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
			<ComposerNativeProvider value={native}>
				<FixedNow value={NOW}>
					<RowProvider value={value}>{children}</RowProvider>
				</FixedNow>
			</ComposerNativeProvider>
		</ServicesProvider>
	);
	return render(wrap(<ItemBodyView item={item} />));
};

const item = (body: ItemBody, status: ItemStatus = "completed") =>
	timelineItem(body, {
		id: `item-${body.kind}`,
		status,
		started_at: NOW - 4000,
		completed_at: status === "completed" ? NOW : null,
	});

const read = (status: ItemStatus = "completed") =>
	item({ kind: "file_read", path: "src/app.ts", lines: null }, status);

const edit = (op: FileEdit["op"], path: string): FileEdit => ({
	path,
	old_path: op === "rename" ? "src/old.ts" : null,
	op,
	additions: 1,
	deletions: 0,
	patch: null,
});

const attachment = (name: string, kind: AttachmentRef["kind"]): AttachmentRef => ({
	id: `att-${name}`,
	name,
	kind,
	bytes: 2048,
	mime: null,
	path: `/staged/${name}`,
	thumbnail: null,
});

describe("downloads", () => {
	it("aren't offered for an agent on this Mac", () => {
		show(read(), { remote: false });
		expect(screen.queryByRole("button", { name: /download/i })).toBeNull();
	});

	it("are offered on files a remote agent read or wrote, once the step is done", () => {
		const { unmount } = show(read("running"));
		expect(screen.queryByRole("button", { name: "Download app.ts" })).toBeNull();
		unmount();

		show(
			item({
				kind: "file_change",
				changes: [
					edit("modify", "src/edited.ts"),
					edit("add", "src/added.ts"),
					edit("delete", "src/deleted.ts"),
					edit("rename", "src/renamed.ts"),
				],
			})
		);
		for (const name of ["edited.ts", "added.ts", "renamed.ts"]) {
			expect(screen.getByRole("button", { name: `Download ${name}` })).toBeInTheDocument();
		}
		expect(screen.queryByRole("button", { name: "Download deleted.ts" })).toBeNull();
	});

	it("show their progress, keep going without their row, then show the file in Finder", async () => {
		const transfers = downloads();
		const revealPath = vi.fn(async () => {});
		const native = { ...browserNative, revealPath };
		const first = show(read(), { source: transfers.source, native });

		fireEvent.click(screen.getByRole("button", { name: "Download app.ts" }));
		expect(transfers.download).toHaveBeenCalledWith("a1", "src/app.ts", expect.any(Function));
		transfers.progress(512);
		const cancel = screen.getByRole("button", { name: "Cancel downloading app.ts" });
		expect(cancel).toHaveTextContent("50%");

		transfers.progress(512, "paused");
		expect(cancel).toHaveAttribute("title", "Paused, reconnecting");

		first.unmount();
		show(read(), { source: transfers.source, native });
		expect(screen.getByRole("button", { name: "Cancel downloading app.ts" })).toHaveTextContent(
			"50%"
		);

		await transfers.finish(OK);
		fireEvent.click(screen.getByRole("button", { name: "Show app.ts in Finder" }));
		expect(revealPath).toHaveBeenCalledWith(LANDED);
	});

	it("say a file landed when the app can't show it in Finder", async () => {
		const transfers = downloads();
		show(read(), { source: transfers.source });
		fireEvent.click(screen.getByRole("button", { name: "Download app.ts" }));
		await transfers.finish(OK);
		expect(screen.getByRole("img", { name: "Downloaded app.ts" })).toHaveAttribute(
			"title",
			LANDED
		);
	});

	it("offer the download again when Finder can't find the file", async () => {
		const transfers = downloads();
		const revealPath = vi.fn(async () => {
			throw new Error("gone");
		});
		show(read(), { source: transfers.source, native: { ...browserNative, revealPath } });
		fireEvent.click(screen.getByRole("button", { name: "Download app.ts" }));
		await transfers.finish(OK);
		await act(async () => {
			fireEvent.click(screen.getByRole("button", { name: "Show app.ts in Finder" }));
		});
		expect(screen.getByRole("alert")).toHaveTextContent("Couldn't find app.ts in Downloads.");
		expect(
			screen.getByRole("button", { name: "Retry downloading app.ts" })
		).toBeInTheDocument();
	});

	it("cancel, even before the transfer has a name", async () => {
		const transfers = downloads();
		show(read(), { source: transfers.source });

		fireEvent.click(screen.getByRole("button", { name: "Download app.ts" }));
		const early = transfers.last();
		fireEvent.click(screen.getByRole("button", { name: "Cancel downloading app.ts" }));
		expect(screen.getByRole("button", { name: "Download app.ts" })).toBeInTheDocument();
		transfers.progress(100);
		transfers.progress(200);
		expect(transfers.cancelTransfer).toHaveBeenCalledTimes(1);
		expect(transfers.cancelTransfer).toHaveBeenCalledWith("t-1");

		fireEvent.click(screen.getByRole("button", { name: "Download app.ts" }));
		transfers.progress(100, "uploading", "t-2");
		fireEvent.click(screen.getByRole("button", { name: "Cancel downloading app.ts" }));
		expect(transfers.cancelTransfer).toHaveBeenLastCalledWith("t-2");

		// What the cancelled transfers send at the end changes nothing.
		await transfers.finish({
			status: "error",
			error: { kind: "failed", message: "Cancelled" },
		});
		await transfers.finish(OK, early);
		expect(screen.getByRole("button", { name: "Download app.ts" })).toBeInTheDocument();
	});

	it("say why one failed, and retry it", async () => {
		const transfers = downloads();
		show(read(), { source: transfers.source });
		fireEvent.click(screen.getByRole("button", { name: "Download app.ts" }));
		await transfers.finish({
			status: "error",
			error: { kind: "not_connected", device: "devbox" },
		});
		expect(screen.getByRole("alert")).toHaveTextContent("devbox isn't connected.");

		fireEvent.click(screen.getByRole("button", { name: "Retry downloading app.ts" }));
		expect(transfers.download).toHaveBeenCalledTimes(2);
		expect(
			screen.getByRole("button", { name: "Cancel downloading app.ts" })
		).toBeInTheDocument();
	});

	it("are offered on the files and pictures sent to a remote agent", () => {
		const transfers = downloads();
		const message = item({
			kind: "user_message",
			text: "Look at these",
			attachments: [attachment("shot.png", "image"), attachment("notes.pdf", "pdf")],
			delivery: "sent",
		});
		const local = show(message, { remote: false, source: transfers.source });
		expect(screen.queryByRole("button", { name: /download/i })).toBeNull();
		local.unmount();

		show(message, { source: transfers.source });
		expect(screen.getByRole("button", { name: "Show shot.png full size" })).toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Download shot.png" }));
		fireEvent.click(screen.getByRole("button", { name: "Download notes.pdf" }));
		expect(transfers.download).toHaveBeenCalledWith(
			"a1",
			"/staged/shot.png",
			expect.any(Function)
		);
		expect(transfers.download).toHaveBeenCalledWith(
			"a1",
			"/staged/notes.pdf",
			expect.any(Function)
		);
	});
});
