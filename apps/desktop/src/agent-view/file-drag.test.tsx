// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { DEVICE, fakeSource, REMOTE } from "@/composer/testing";
import type { AttachmentRef, DeviceId } from "@maxhq/shared/bindings";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { agent, entities, NOW, workspace } from "@maxhq/shared/test/build";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";
import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";

import { type DraggedFile, FILE_DRAG_TYPE, readDraggedFile, startFileDrag } from "./file-drag";
import { ItemBodyView } from "./item-row";

const FILE: DraggedFile = {
	device: REMOTE,
	agent: "a1",
	path: "/attachments/a1/shot.png",
	name: "shot.png",
	kind: "image",
	mime: "image/png",
	bytes: 48_213,
};

/** A drag's data, keeping what's put on it. */
const transfer = () => {
	const data = new Map<string, string>();
	return {
		data,
		dataTransfer: {
			effectAllowed: "all",
			setData: vi.fn((type: string, value: string) => data.set(type, value)),
		},
	};
};

const sent = (name: string, kind: AttachmentRef["kind"], mime: string): AttachmentRef => ({
	id: `att-${name}`,
	name,
	kind,
	bytes: 48_213,
	mime,
	path: `/attachments/a1/${name}`,
	thumbnail: kind === "image" ? "data:image/jpeg;base64,/9j/4AAQSkZJRg==" : null,
});

/** A message with `files` from agent "a1", whose folder is on `device`, if any. */
const showSent = async (files: AttachmentRef[], device: DeviceId | null) => {
	const { source } = fakeSource({ agent: agent({ id: "a1" }) });
	const snapshot = async () => ({
		seq: 1,
		entities: entities({
			agents: [agent({ id: "a1" })],
			workspaces: device ? [workspace({ id: "w1", device })] : [],
		}),
	});
	const row: RowContext = {
		agent: "a1",
		vendor: "claude",
		timeline: createTimelineStore(),
		readOnly: false,
		remote: device === REMOTE,
		answer: async () => ({ status: "ok", data: { status: "done" } }),
		subAgentsShown: false,
		toggleSubAgents: () => {},
	};
	const message = timelineItem(
		{ kind: "user_message", text: "Look", attachments: files, delivery: "sent" },
		{ id: "item-1", status: "completed", started_at: NOW - 4000, completed_at: NOW }
	);
	render(
		<ServicesProvider source={{ ...source, snapshot }}>
			<FixedNow value={NOW}>
				<RowProvider value={row}>
					<ItemBodyView item={message} />
				</RowProvider>
			</FixedNow>
		</ServicesProvider>
	);
	// The view loads after a turn.
	await act(() => new Promise((resolve) => setTimeout(resolve, 0)));
};

describe("readDraggedFile", () => {
	it("reads back what a drag carries", () => {
		const { data, dataTransfer } = transfer();
		startFileDrag(dataTransfer as unknown as DataTransfer, FILE);
		expect(dataTransfer.effectAllowed).toBe("copy");
		expect(data.get("text/plain")).toBe("shot.png");
		expect(readDraggedFile(data.get(FILE_DRAG_TYPE))).toEqual(FILE);
	});

	it("takes nothing that isn't a whole file", () => {
		expect(readDraggedFile(undefined)).toBeNull();
		expect(readDraggedFile("")).toBeNull();
		expect(readDraggedFile("{not json")).toBeNull();
		expect(readDraggedFile("null")).toBeNull();
		expect(readDraggedFile(JSON.stringify({ ...FILE, path: "" }))).toBeNull();
		expect(readDraggedFile(JSON.stringify({ ...FILE, device: undefined }))).toBeNull();
		expect(readDraggedFile(JSON.stringify({ ...FILE, kind: "video" }))).toBeNull();
		expect(readDraggedFile(JSON.stringify({ ...FILE, bytes: "48 KB" }))).toBeNull();
	});
});

describe("dragging a file out of a timeline", () => {
	it("carries a picture you sent from the agent's device", async () => {
		await showSent([sent("shot.png", "image", "image/png")], REMOTE);
		const thumb = screen.getByRole("button", { name: "Show shot.png full size" });
		// Only the frame drags, so the webview doesn't drag the preview on its own.
		expect(thumb).toHaveAttribute("draggable", "true");
		expect(thumb.querySelector("img")).toHaveAttribute("draggable", "false");

		const { data, dataTransfer } = transfer();
		fireEvent.dragStart(thumb, { dataTransfer });
		expect(readDraggedFile(data.get(FILE_DRAG_TYPE))).toEqual(FILE);
		expect(data.get("text/plain")).toBe("shot.png");
	});

	it("carries a file you sent", async () => {
		await showSent([sent("notes.pdf", "pdf", "application/pdf")], DEVICE);
		const chip = screen.getByTitle("/attachments/a1/notes.pdf");
		const { data, dataTransfer } = transfer();
		fireEvent.dragStart(chip, { dataTransfer });
		expect(readDraggedFile(data.get(FILE_DRAG_TYPE))).toMatchObject({
			device: DEVICE,
			agent: "a1",
			path: "/attachments/a1/notes.pdf",
			kind: "pdf",
		});
	});

	it("starts no drag while the agent's device isn't known", async () => {
		await showSent([sent("shot.png", "image", "image/png")], null);
		const { dataTransfer } = transfer();
		const went = fireEvent.dragStart(
			screen.getByRole("button", { name: "Show shot.png full size" }),
			{ dataTransfer }
		);
		expect(went).toBe(false);
		expect(dataTransfer.setData).not.toHaveBeenCalled();
	});

	it("closes the full size picture once it drags out", async () => {
		await showSent([sent("shot.png", "image", "image/png")], REMOTE);
		fireEvent.click(screen.getByRole("button", { name: "Show shot.png full size" }));
		const dialog = await screen.findByRole("dialog");
		const picture = dialog.querySelector("[draggable=true]");
		if (!picture) throw new Error("the picture doesn't drag");
		const { data, dataTransfer } = transfer();
		fireEvent.dragStart(picture, { dataTransfer });
		expect(readDraggedFile(data.get(FILE_DRAG_TYPE))).toEqual(FILE);
		// It closes after the drag has started, so the drag keeps its element.
		expect(dialog.querySelector("[draggable=true]")).toBe(picture);
		await waitFor(() => expect(dialog).toHaveAttribute("data-closed"));
	});
});
