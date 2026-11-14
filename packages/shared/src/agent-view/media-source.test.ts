// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the cases for
// resolving a media source, with cases for what MaxHQ does differently: a
// relative path goes to the device as written, and a share, a home folder
// or another scheme shows nothing.

import { describe, expect, it } from "vitest";

import { resolveMediaSource } from "./media-source";

describe("resolveMediaSource", () => {
	describe("URLs", () => {
		it.each(["https://cdn.example.com/tone.mp3", "data:audio/wav;base64,AAAA"])(
			"keeps %s out of the image and video viewer",
			(href) => {
				expect(resolveMediaSource(href)).toBeNull();
				expect(resolveMediaSource(href, { imageEmbed: true })).toBeNull();
			}
		);

		it("keeps the authored URL and decodes the display name once", () => {
			const href = "https://cdn.example.com/clip%20one%2520%2Emp4?signature=a%2fb#t=2";
			expect(resolveMediaSource(href)).toEqual({
				kind: "video",
				mime: "video/mp4",
				name: "clip one%20.mp4",
				fragment: "#t=2",
				access: "web",
				uri: href,
			});
		});

		it("accepts extensionless image embeds only when asked", () => {
			const href = "https://cdn.example.com/render?id=42";
			expect(resolveMediaSource(href)).toBeNull();
			expect(resolveMediaSource(href, { imageEmbed: true })).toMatchObject({
				kind: "image",
				mime: "image/*",
				name: "render",
				access: "web",
			});
		});

		it("shows a data picture as it is, and never a data video", () => {
			const href = "data:image/png;base64,iVBORw0KGgo=";
			expect(resolveMediaSource(href)).toEqual({
				kind: "image",
				mime: "image/png",
				name: "image",
				fragment: "",
				access: "inline",
				uri: href,
			});
			expect(resolveMediaSource("data:video/mp4;base64,AAAA")).toBeNull();
			expect(resolveMediaSource("data:text/html,<p>hi</p>", { imageEmbed: true })).toBeNull();
		});

		it("reads a protocol-relative URL as https", () => {
			expect(resolveMediaSource("//cdn.example.com/a.png")).toMatchObject({
				access: "web",
				uri: "https://cdn.example.com/a.png",
			});
		});
	});

	describe("device paths", () => {
		it.each(["tone.wav", "tone.mp3", "tone.oga", "tone.webm-audio"])(
			"keeps device audio %s out of the image fallback",
			(path) => {
				expect(resolveMediaSource(path, { imageEmbed: true })).toBeNull();
			}
		);

		it.each([
			["/tmp/frame%23one.png:12", "/tmp/frame#one.png"],
			["/tmp/frame%2523one.png:12", "/tmp/frame%23one.png"],
			["file:///tmp/frame%20one.png", "/tmp/frame one.png"],
			["file://localhost/tmp/a.png", "/tmp/a.png"],
			["<./shots/a b.png>", "./shots/a b.png"],
			["file:///C:/shots/a.png", "C:/shots/a.png"],
		])("resolves %s to the device's %s", (href, path) => {
			expect(resolveMediaSource(href)).toEqual({
				kind: "image",
				mime: "image/png",
				name: path.split(/[\\/]/).at(-1),
				fragment: "",
				access: "device",
				path,
			});
		});

		it("sends a relative path as written, without its search", () => {
			expect(resolveMediaSource("screens/logo.svg?v=2#mark")).toEqual({
				kind: "image",
				mime: "image/svg+xml",
				name: "logo.svg",
				fragment: "#mark",
				access: "device",
				path: "screens/logo.svg",
			});
		});

		it("separates a playback fragment from literal filename characters", () => {
			expect(resolveMediaSource("/tmp/clip%23one.mp4#t=2")).toMatchObject({
				kind: "video",
				mime: "video/mp4",
				fragment: "#t=2",
				path: "/tmp/clip#one.mp4",
			});
		});

		it("reads the extension whatever its case", () => {
			expect(resolveMediaSource("/w/Shot.PNG")).toMatchObject({
				kind: "image",
				name: "Shot.PNG",
			});
			expect(resolveMediaSource("/w/Clip.MOV")).toMatchObject({ kind: "video" });
		});

		it.each([
			"\\\\server\\share\\a.png",
			"file://server/share/a.png",
			"~/Pictures/a.png",
			"ftp://example.com/a.png",
			"blob:https://example.com/abc",
			"javascript:alert(1)//a.png",
			"#a.png",
			"?a.png",
			"",
			"/w/notes.txt",
			"/w/clip.mkv",
			"/w/render",
		])("shows nothing for %s", (href) => {
			expect(resolveMediaSource(href, { imageEmbed: true })).toBeNull();
		});

		it("shows nothing for a missing target", () => {
			expect(resolveMediaSource(null)).toBeNull();
			expect(resolveMediaSource(undefined)).toBeNull();
		});
	});
});
