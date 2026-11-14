// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { readVisualTransfer, writeVisualTransfer } from "./transfer";
import { MAX_VISUAL_BYTES } from "./visual";

describe("visual transfers", () => {
	it("read back what they wrote", () => {
		const visual = { html: "<p>hi</p>", title: "Hi", from: "Token audit on devbox" };
		expect(readVisualTransfer(writeVisualTransfer(visual))).toEqual(visual);
		const bare = { ...visual, from: null };
		expect(readVisualTransfer(writeVisualTransfer(bare))).toEqual(bare);
	});

	it("refuse text that isn't a visual", () => {
		for (const text of [
			"",
			"not json",
			"null",
			"[]",
			'{"title":"x","from":null}',
			'{"html":1,"title":"x","from":null}',
			'{"html":"<p></p>","from":null}',
			'{"html":"<p></p>","title":"x","from":3}',
		])
			expect(readVisualTransfer(text)).toBeNull();
	});

	it("refuse a page over the size limit", () => {
		const html = "x".repeat(MAX_VISUAL_BYTES + 1);
		expect(readVisualTransfer(writeVisualTransfer({ html, title: "", from: null }))).toBeNull();
		const fits = "x".repeat(MAX_VISUAL_BYTES);
		expect(
			readVisualTransfer(writeVisualTransfer({ html: fits, title: "", from: null }))
		).not.toBeNull();
	});
});
