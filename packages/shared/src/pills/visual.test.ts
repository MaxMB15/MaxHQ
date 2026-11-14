// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { toVisualFence } from "../visuals/fence";
import { VISUAL_MIME, writeVisualTransfer } from "../visuals/transfer";
import { MAX_VISUAL_BYTES } from "../visuals/visual";
import {
	decodeClipboardHtml,
	decodeFragment,
	encodeClipboardHtml,
	encodeFragment,
} from "./clipboard";
import { type Draft, fileKeys, fromUserInput, plainText, toUserInput } from "./draft";
import { readVisual, VISUAL_KEY, visualFrom, visualFragment } from "./visual";

const page = { html: "<p>Weekly</p>", title: "Weekly usage", from: "Token audit on Mac mini" };

const transfer = (types: Record<string, string>) => ({
	getData: (type: string) => types[type] ?? "",
});

describe("the visual pill", () => {
	it("names where it came from", () => {
		expect(visualFrom("Token audit", "Mac mini")).toBe("Token audit on Mac mini");
		expect(visualFrom("Token audit", null)).toBe("Token audit");
	});

	it("goes as a visual, or as its page to an older maxd", () => {
		const draft: Draft = {
			segments: [
				{ type: "text", text: "Thinner bars on " },
				{
					type: "pill",
					pill: { type: "visual", key: "k", title: "Weekly usage", from: null },
				},
			],
			gitItems: [],
		};
		expect(fileKeys(draft.segments)).toEqual(["k"]);
		const idOf = (key: string) => (key === "k" ? "att-page" : null);
		const sent = toUserInput(draft, idOf);
		expect(sent.attachments).toEqual(["att-page"]);
		expect(sent.refs).toEqual([
			{ type: "visual", attachment: "att-page", title: "Weekly usage", from: null },
		]);
		expect(toUserInput(draft, idOf, { visuals: false }).refs).toEqual([
			{ type: "attachment", id: "att-page" },
		]);
		// Brought back to edit, it's a visual pill again.
		expect(fromUserInput(sent, () => "k").segments).toEqual(draft.segments);
		// A page that hasn't staged drops out.
		expect(toUserInput(draft, () => null).attachments).toEqual([]);
	});

	it("writes its fenced block on lines of its own, or its marker without the page", () => {
		const segments = visualFragment(page).segments;
		const around = [
			{ type: "text" as const, text: "See " },
			...segments,
			{ type: "text" as const, text: " now" },
		];
		const fence = toVisualFence(page);
		const pageOf = () => page.html;
		expect(plainText(around, () => undefined, pageOf)).toBe(`See \n${fence}\n now`);
		expect(plainText(segments, () => undefined, pageOf)).toBe(fence);
		expect(plainText(around, () => undefined)).toBe("See [Visual: Weekly usage] now");
	});

	it("round trips through the draft type and its HTML with the page", () => {
		const fragment = visualFragment(page, "composer-1");
		expect(fragment.files).toEqual([
			{
				key: VISUAL_KEY,
				name: "visual-weekly-usage.html",
				kind: "file",
				source: null,
				ref: null,
				device: null,
				page: page.html,
			},
		]);
		const encoded = encodeFragment(fragment) ?? "";
		expect(decodeFragment(encoded)).toEqual(fragment);
		// From HTML, any app could have written it: the page stays, the rest goes.
		expect(decodeClipboardHtml(encodeClipboardHtml(toVisualFence(page), encoded))).toEqual(
			fragment
		);
		const pageless = { ...fragment.files[0]!, page: undefined };
		const without = encodeFragment({ ...fragment, files: [pageless] }) ?? "";
		const decoded = decodeClipboardHtml(encodeClipboardHtml("", without));
		expect(decoded?.segments).toEqual([{ type: "text", text: "[Visual: Weekly usage]" }]);
		expect(decoded?.files).toEqual([]);
	});

	it("reads a paste under its own type, or plain text that is one fenced block", () => {
		expect(readVisual(transfer({ [VISUAL_MIME]: writeVisualTransfer(page) }))).toEqual(page);
		expect(readVisual(transfer({ "text/plain": `\n${toVisualFence(page)}\n` }))).toEqual({
			...page,
			from: null,
		});
		expect(readVisual(transfer({ "text/plain": `Look:\n${toVisualFence(page)}` }))).toBeNull();
		expect(readVisual(transfer({ "text/plain": "```js\nx\n```" }))).toBeNull();
		const large = { html: "x".repeat(MAX_VISUAL_BYTES + 1), title: "Big" };
		expect(readVisual(transfer({ "text/plain": toVisualFence(large) }))).toBeNull();
		expect(
			readVisual(transfer({ [VISUAL_MIME]: writeVisualTransfer({ ...large, from: null }) }))
		).toBeNull();
	});
});
