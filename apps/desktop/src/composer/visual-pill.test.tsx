// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { VISUALS_SINCE } from "@maxhq/shared/bindings";
import { DRAFT_CLIPBOARD_MIME, encodeFragment } from "@maxhq/shared/pills/clipboard";
import { OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";
import { visualFragment } from "@maxhq/shared/pills/visual";
import { link } from "@maxhq/shared/test/build";
import { toVisualFence } from "@maxhq/shared/visuals/fence";
import { VISUAL_MIME, writeVisualTransfer } from "@maxhq/shared/visuals/transfer";
import { MAX_VISUAL_BYTES } from "@maxhq/shared/visuals/visual";

import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import {
	AGENT,
	DEVICE,
	editorOf,
	findTextbox,
	installBrowserFakes,
	pillsIn,
	renderComposer,
	textIn,
} from "./testing";

beforeAll(installBrowserFakes);
afterEach(() => {
	quoteInbox.take(AGENT);
});

const visual = { html: "<p>Weekly</p>", title: "Weekly usage", from: "Token audit on Mac" };

/** A clipboard that holds `types` for a paste to read, and keeps what a copy writes. */
const clipboardWith = (types: Record<string, string> = {}) => {
	const data = new Map(Object.entries(types));
	return {
		data,
		setData: (format: string, value: string) => data.set(format, value),
		clearData: () => data.clear(),
		getData: (format: string) => data.get(format) ?? "",
		files: [] as File[],
		get types() {
			return [...data.keys()];
		},
	};
};

const pasteIn = (textbox: HTMLElement, types: Record<string, string>) =>
	fireEvent.paste(textbox, { clipboardData: clipboardWith(types) });

const VISUAL_PILL = /^Visual Weekly usage/;

/** The Mac's own link, its maxd at `protocol`. */
const macAt = (protocol: number) => link({ id: "mac", device: DEVICE, protocol });

describe("the visual pill", () => {
	it.each([
		["the visual's own type", { [VISUAL_MIME]: writeVisualTransfer(visual) }],
		[
			"the draft's type",
			{ [DRAFT_CLIPBOARD_MIME]: encodeFragment(visualFragment(visual, "elsewhere"))! },
		],
		["plain text that is one fenced block", { "text/plain": toVisualFence(visual) }],
	])("comes from a paste of %s, its page staged as a file", async (_, types) => {
		const { source } = renderComposer();
		const textbox = await findTextbox();
		pasteIn(textbox, types);
		await waitFor(() => expect(pillsIn(textbox)).toEqual([expect.stringMatching(VISUAL_PILL)]));
		expect(textIn(textbox)).toBe(`${MARK} `);
		expect(source.stagePasted).toHaveBeenCalledWith(
			DEVICE,
			"visual-weekly-usage.html",
			"text/html",
			expect.any(Uint8Array)
		);
	});

	it("leaves a fenced page over the limit as text", async () => {
		renderComposer();
		const textbox = await findTextbox();
		const large = toVisualFence({ html: "x".repeat(MAX_VISUAL_BYTES + 1), title: "Big" });
		pasteIn(textbox, { "text/plain": large });
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));
		expect(pillsIn(textbox)).not.toContainEqual(expect.stringMatching(/^Visual /));
	});

	it("goes as a visual to a maxd that knows them, and as its file to an older one", async () => {
		for (const [protocol, ref] of [
			[
				VISUALS_SINCE,
				{
					type: "visual",
					attachment: "att-visual-weekly-usage.html",
					title: visual.title,
					from: visual.from,
				},
			],
			[VISUALS_SINCE - 1, { type: "attachment", id: "att-visual-weekly-usage.html" }],
		] as const) {
			const { source } = renderComposer({ links: [macAt(protocol)] });
			const textbox = await findTextbox();
			pasteIn(textbox, { [VISUAL_MIME]: writeVisualTransfer(visual) });
			await waitFor(() =>
				expect(pillsIn(textbox)).toEqual([expect.stringMatching(VISUAL_PILL)])
			);
			fireEvent.keyDown(textbox, { key: "Enter" });
			await waitFor(() =>
				expect(source.sendInput).toHaveBeenCalledWith(
					AGENT,
					expect.objectContaining({
						attachments: ["att-visual-weekly-usage.html"],
						refs: [ref],
					}),
					"queue"
				)
			);
			cleanup();
		}
	});

	it("copies as its fenced block, the draft's type and the visual's type", async () => {
		renderComposer();
		const textbox = await findTextbox();
		pasteIn(textbox, { [VISUAL_MIME]: writeVisualTransfer(visual) });
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(1));
		const clipboard = clipboardWith();
		act(() => {
			editorOf(textbox).commands.selectAll();
		});
		fireEvent.copy(textbox, { clipboardData: clipboard });
		expect(clipboard.data.get("text/plain")).toBe(`${toVisualFence(visual)}\n `);
		expect(clipboard.data.get(DRAFT_CLIPBOARD_MIME)).toContain("Weekly usage");
		expect(clipboard.data.get("text/html")).toContain("Weekly usage");
		expect(JSON.parse(clipboard.data.get(VISUAL_MIME) ?? "")).toEqual(visual);
	});

	it("comes from Add to chat and from a drop on the composer", async () => {
		const { drop } = renderComposer();
		const textbox = await findTextbox();
		act(() => quoteInbox.sendVisual(AGENT, visual, true));
		await waitFor(() => expect(pillsIn(textbox)).toEqual([expect.stringMatching(VISUAL_PILL)]));

		const drag = new Map([[VISUAL_MIME, writeVisualTransfer(visual)]]);
		expect(drop().takesPageDrag?.(drag, { x: 0, y: 0 })).toBe(true);
		act(() => drop().onPageDrop?.(drag, { x: 0, y: 0 }));
		await waitFor(() => expect(pillsIn(textbox)).toHaveLength(2));
	});
});
