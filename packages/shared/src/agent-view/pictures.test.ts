// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { morePictures, PICTURES_SHOWN, previewOf, shownPictures } from "./pictures";

describe("previewOf", () => {
	it("draws a preview as maxd makes one", () => {
		for (const thumbnail of [
			"data:image/jpeg;base64,/9j/4AAQSkZJRg==",
			"data:image/png;base64,iVBORw0KGgo=",
		]) {
			expect(previewOf({ thumbnail })).toBe(thumbnail);
		}
	});

	it("leaves out any other address, so the plain frame shows", () => {
		for (const thumbnail of [
			null,
			"",
			"https://example.com/track.png",
			"//example.com/track.png",
			"file:///Users/me/secret.png",
			"maxmedia://localhost/frame.png",
			"blob:https://example.com/1",
			"data:image/png;base64,",
			"data:image/svg+xml;base64,PHN2Zz4=",
			"data:image/gif;base64,R0lGODlh",
			"DATA:IMAGE/PNG;BASE64,iVBORw0KGgo=",
			"data:image/png,iVBORw0KGgo=",
			'data:image/png;base64,ab"cd',
			"data:image/png;base64,iVBORw0KGgo=\nhttps://example.com",
			" data:image/png;base64,iVBORw0KGgo=",
		]) {
			expect(previewOf({ thumbnail }), String(thumbnail)).toBeNull();
		}
	});

	it("leaves out one longer than maxd makes", () => {
		const head = "data:image/png;base64,";
		const most = `${head}${"A".repeat(48 * 1024 - head.length)}`;
		expect(previewOf({ thumbnail: most })).toBe(most);
		expect(previewOf({ thumbnail: `${most}A` })).toBeNull();
	});
});

describe("shownPictures", () => {
	const pictures = (count: number) => Array.from({ length: count }, (_, i) => i);

	it("shows every picture of a usual block", () => {
		expect(shownPictures(pictures(3), false)).toEqual({ shown: [0, 1, 2], more: 0 });
		expect(shownPictures(pictures(PICTURES_SHOWN), false)).toEqual({
			shown: pictures(PICTURES_SHOWN),
			more: 0,
		});
	});

	it("keeps the rest of a long block for a tap", () => {
		const many = pictures(PICTURES_SHOWN + 300);
		expect(shownPictures(many, false)).toEqual({
			shown: pictures(PICTURES_SHOWN),
			more: 300,
		});
		expect(shownPictures(many, true)).toEqual({ shown: many, more: 0 });
	});

	it("says how many the tap shows", () => {
		expect(morePictures(1)).toBe("Show 1 more picture");
		expect(morePictures(300)).toBe("Show 300 more pictures");
	});
});
