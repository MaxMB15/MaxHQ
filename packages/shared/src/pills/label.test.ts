// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	fileAria,
	fileLabel,
	gitItemLabel,
	gitStateWords,
	middleTruncate,
	quoteLabel,
} from "./label";

describe("pill labels", () => {
	it("cuts the middle out of a long name and keeps the extension", () => {
		const long = "a-very-long-screenshot-name-from-yesterday-evening.png";
		const cut = middleTruncate(long);
		expect(Array.from(cut)).toHaveLength(36);
		expect(cut.endsWith("evening.png")).toBe(true);
		expect(cut).toContain("…");
		expect(middleTruncate("shot.png")).toBe("shot.png");
	});

	it("calls a pasted picture by its number and a folder with a slash", () => {
		expect(fileLabel({ name: "pasted-image-2.png", kind: "image" }, 3)).toBe("Image 3");
		expect(fileLabel({ name: "shot.png", kind: "image" }, 1)).toBe("shot.png");
		expect(fileLabel({ name: "src", kind: "folder" }, undefined)).toBe("src/");
	});

	it("shows a quote's first words", () => {
		expect(quoteLabel("short one")).toBe("“short one”");
		expect(quoteLabel("the cache misses on the second poll every time")).toBe(
			"“the cache misses on the…”"
		);
	});

	it("names a pull request and its state", () => {
		expect(gitItemLabel({ number: 412, title: "Cache usage" })).toBe("#412 Cache usage");
		expect(gitStateWords("not_planned")).toBe("not planned");
	});

	it("tells a screen reader everything the pill shows", () => {
		const shot = { name: "shot.png", kind: "image" } as const;
		expect(fileAria(shot, 1, "240 KB", { state: "uploading", percent: 40 })).toBe(
			"Image 1, shot.png, 240 KB, uploading, 40 percent"
		);
		expect(
			fileAria({ name: "main.rs", kind: "file" }, undefined, null, {
				state: "failed",
				error: "no space left",
			})
		).toBe("File, main.rs, failed, no space left");
		expect(fileAria(shot, 2, null, { state: "ready" })).toBe("Image 2, shot.png");
	});
});
