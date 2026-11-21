// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	blockedLoad,
	HEIGHT_START,
	type HeightTrack,
	isPlainHost,
	nextHeight,
	NO_PEERS,
	readFrameReport,
	readShowVisual,
	visualDocument,
	visualFrameQuery,
	withoutWarmUps,
} from "./frame";

const follow = (heights: number[], track: HeightTrack = HEIGHT_START): HeightTrack =>
	heights.reduce(nextHeight, track);

describe("visual frame messages", () => {
	it("reads each report the frame sends", () => {
		expect(readFrameReport({ type: "maxhq-visual:ready" })).toEqual({
			type: "maxhq-visual:ready",
		});
		expect(readFrameReport({ type: "maxhq-visual:height", height: 120.5 })).toEqual({
			type: "maxhq-visual:height",
			height: 120.5,
		});
		expect(readFrameReport({ type: "maxhq-visual:error", message: "x", line: 3.5 })).toEqual({
			type: "maxhq-visual:error",
			message: "x",
			line: null,
		});
		expect(
			readFrameReport({
				type: "maxhq-visual:blocked",
				url: "https://a.b/c",
				what: "script-src",
			})
		).toEqual({ type: "maxhq-visual:blocked", url: "https://a.b/c", what: "script-src" });
		expect(readFrameReport({ type: "maxhq-visual:missing", path: "a.png" })).toEqual({
			type: "maxhq-visual:missing",
			path: "a.png",
		});
		expect(readFrameReport({ type: "maxhq-visual:link", url: "https://a.b" })?.type).toBe(
			"maxhq-visual:link"
		);
	});

	it("drops anything malformed", () => {
		for (const data of [
			null,
			"maxhq-visual:ready",
			{ type: "maxhq-viewer:ready" },
			{ type: "maxhq-visual:height", height: "1" },
			{ type: "maxhq-visual:height", height: Number.NaN },
			{ type: "maxhq-visual:error" },
			{ type: "maxhq-visual:blocked", url: "x" },
			{ type: "maxhq-visual:link", url: "x".repeat(5000) },
		]) {
			expect(readFrameReport(data)).toBeNull();
		}
		expect(
			readShowVisual({ type: "maxhq-visual:show", html: "", theme: "dim", variables: {} })
		).toBeNull();
		expect(
			readShowVisual({ type: "maxhq-visual:show", html: "", theme: "dark", variables: {} })
		).not.toBeNull();
	});
});

describe("the visual's document", () => {
	it("puts the theme first and points libraries at the frame's copies", () => {
		const document = visualDocument({
			html: '<!DOCTYPE html><html><script src="maxhq:lib/chart.js"></script></html>',
			theme: "dark",
			variables: {
				"--chart-1": "#4692f6",
				"--foreground": "red;}</style><script>",
				"--other": "x",
			},
		});
		expect(document).toContain("<style>:root{color-scheme:dark;");
		expect(document).toContain("--foreground:red/style");
		expect(document).toContain("--chart-1:#4692f6");
		expect(document).not.toContain("--other");
		expect(document).toContain('<html><script src="/visual-lib/chart.js">');
		expect(document).not.toContain("<!DOCTYPE");
	});

	it("turns off name lookups and takes peer connections away before the page runs", () => {
		const document = visualDocument({
			html: "<script>new RTCPeerConnection()</script>",
			theme: "light",
			variables: {},
		});
		expect(
			document.startsWith(
				`<!doctype html><meta http-equiv="x-dns-prefetch-control" content="off"><script>${NO_PEERS}</script>`
			)
		).toBe(true);
		const page = {
			RTCPeerConnection: 1,
			webkitRTCPeerConnection: 2,
			RTCDataChannel: 3,
			fetch: 4,
		};
		new Function("window", NO_PEERS)(page);
		expect(page).toEqual({ fetch: 4 });
		// A name it can't take away doesn't stop the rest.
		const locked = Object.defineProperty({ RTCDataChannel: 3 }, "RTCPeerConnection", {
			value: 1,
		});
		new Function("window", `"use strict";${NO_PEERS}`)(locked);
		expect(locked).toEqual({});
		expect("RTCPeerConnection" in locked).toBe(true);
	});

	it("drops the links that would reach out ahead of a load", () => {
		const page = [
			'<link rel="stylesheet" href="/visual-lib/x.css">',
			'<link rel="preconnect" href="https://a.example.com">',
			"<LINK REL=dns-prefetch HREF=//b.example.com>",
			"<link/rel=prefetch href=https://c.example.com/>",
			"<link rel='prerender' href='https://d.example.com'>",
			'<link title="a > b" rel="preconnect" href="https://e.example.com">',
			'<link rel="pre&#99;onnect" href="https://f.example.com">',
			"<linkbox>preconnect</linkbox>",
			"<p>prefetch</p>",
		].join("");
		expect(withoutWarmUps(page)).toBe(
			'<link rel="stylesheet" href="/visual-lib/x.css"><linkbox>preconnect</linkbox><p>prefetch</p>'
		);
		// One left open runs to the end of the page.
		expect(withoutWarmUps('<p>a</p><link rel="preconnect" href="https://g.example.com"')).toBe(
			"<p>a</p>"
		);
		expect(
			visualDocument({
				html: '<link rel="preconnect" href="https://a.example.com"><p>hi</p>',
				theme: "dark",
				variables: {},
			})
		).not.toContain("a.example.com");
	});
});

describe("the frame's height", () => {
	it("follows the page, capped at 20,000 pixels", () => {
		expect(follow([120.2]).height).toBe(121);
		expect(follow([120, 300, 80]).height).toBe(80);
		expect(follow([50_000]).height).toBe(20_000);
	});

	it("stops following a page that keeps growing by the same step", () => {
		const loop = Array.from({ length: 20 }, (_, i) => 300 + i * 16);
		const track = follow(loop);
		expect(track.stopped).toBe(true);
		expect(track.height).toBe(300);
		expect(nextHeight(track, 900)).toBe(track);
	});

	it("keeps following steps that differ, as an opening accordion's do", () => {
		const track = follow([300, 340, 390, 400, 430, 431, 470, 480, 500, 900]);
		expect(track.stopped).toBe(false);
		expect(track.height).toBe(900);
	});
});

describe("hosts a visual may load from", () => {
	it("takes only plain https host names with an optional port", () => {
		for (const host of ["cdn.example.com", "a-b.example.co.uk", "cdn.example.com:8443"]) {
			expect(isPlainHost(host), host).toBe(true);
		}
		for (const host of [
			"localhost",
			"127.0.0.1",
			"example",
			"Example.com",
			"*.example.com",
			"-a.example.com",
			"a..example.com",
			"example.com:0",
			"example.com:99999",
			"example.com:443:1",
			"example.com/x",
			"example.com 'unsafe-inline'",
			"example.com;script-src",
			"[::1]",
		]) {
			expect(isPlainHost(host), host).toBe(false);
		}
	});

	it("says which blocked loads Load this time can help", () => {
		expect(blockedLoad("https://cdn.jsdelivr.net/npm/x.js", "script-src-elem")).toEqual({
			host: "cdn.jsdelivr.net",
			loadable: true,
		});
		expect(blockedLoad("https://api.example.com/data", "connect-src")?.loadable).toBe(false);
		expect(blockedLoad("http://cdn.example.com/x.js", "script-src")?.loadable).toBe(false);
		expect(blockedLoad("https://10.0.0.1/x.js", "script-src")?.loadable).toBe(false);
		expect(blockedLoad("inline", "script-src")).toBeNull();
	});

	it("puts the theme and the plain hosts in the frame's query", () => {
		expect(visualFrameQuery("dark", [])).toBe("?theme=dark");
		expect(visualFrameQuery("light", ["a.example.com", "bad host", "b.example.com:8443"])).toBe(
			"?theme=light&hosts=a.example.com,b.example.com:8443"
		);
	});
});
