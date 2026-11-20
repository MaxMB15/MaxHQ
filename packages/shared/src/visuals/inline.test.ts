// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { MOST_HOSTS } from "./frame";
import {
	INLINE_VISUAL_POLICY,
	inlineVisualDocument,
	inlineVisualPolicy,
	opensFromVisual,
	readBridgeReport,
} from "./inline";

const LIBRARIES = new Map([
	["chart.js", "window.Chart = 1;"],
	["d3.js", "window.d3 = '</script>';"],
]);

const make = (html: string, hosts?: readonly string[]) =>
	inlineVisualDocument(
		{ html, theme: "dark", variables: { "--background": "#0f1217", "--chart-1": "#4692f6" } },
		LIBRARIES,
		hosts
	);

describe("a visual drawn with no server", () => {
	it("keeps the page off the network with a policy at the top", () => {
		const { document } = make("<p>hi</p>");
		expect(
			document.startsWith('<!doctype html><meta http-equiv="Content-Security-Policy"')
		).toBe(true);
		expect(document).toContain(`content="${INLINE_VISUAL_POLICY}"`);
		expect(INLINE_VISUAL_POLICY).toContain("default-src 'none'");
		expect(INLINE_VISUAL_POLICY).toContain("connect-src 'none'");
		expect(INLINE_VISUAL_POLICY).not.toMatch(/https?:/);
		// The policy and the reporter come before anything of the page's.
		expect(document.indexOf("Content-Security-Policy")).toBeLessThan(document.indexOf("<p>hi"));
		expect(document.indexOf("ReactNativeWebView")).toBeLessThan(document.indexOf("<p>hi"));
	});

	it("lets through code, styles, fonts and pictures from the hosts it's given, and no more", () => {
		const policy = inlineVisualPolicy(["cdn.example.com", "fonts.example.org:8443"]);
		const directives = policy.split("; ");
		for (const kind of ["script-src", "style-src", "img-src", "font-src"]) {
			const directive = directives.find((d) => d.startsWith(`${kind} `)) ?? "";
			expect(directive).toContain(" https://cdn.example.com");
			expect(directive).toContain(" https://fonts.example.org:8443");
		}
		for (const kind of ["connect-src", "frame-src", "media-src", "worker-src", "default-src"])
			expect(directives.find((d) => d.startsWith(`${kind} `))).not.toMatch(/https?:/);
		expect(make("<p>hi</p>", ["cdn.example.com"]).document).toContain(
			`content="${inlineVisualPolicy(["cdn.example.com"])}"`
		);
	});

	it("takes only plain hosts, and only the first few", () => {
		expect(inlineVisualPolicy([])).toBe(INLINE_VISUAL_POLICY);
		expect(
			inlineVisualPolicy(["localhost", "127.0.0.1", "*.example.com", "a.example.com; x"])
		).toBe(INLINE_VISUAL_POLICY);
		const many = ["a.example.com", "b.example.com", "c.example.com", "d.example.com"];
		const policy = inlineVisualPolicy([...many, "e.example.com"]);
		expect(policy).toContain("https://d.example.com");
		expect(policy).not.toContain("e.example.com");
		expect(MOST_HOSTS).toBe(many.length);
	});

	it("reports how tall the page is, at the view's width", () => {
		const { document } = make("<p>hi</p>");
		expect(document).toContain(
			'<meta name="viewport" content="width=device-width, initial-scale=1">'
		);
		expect(document.indexOf('name="viewport"')).toBeLessThan(document.indexOf("<p>hi"));
		expect(document).toContain('type: "maxhq-visual:height"');
		expect(document.indexOf("maxhq-visual:height")).toBeLessThan(document.indexOf("<p>hi"));
	});

	it("draws in the theme it's given", () => {
		const { document } = make("<p>hi</p>");
		expect(document).toContain("color-scheme:dark;--background:#0f1217;--chart-1:#4692f6");
	});

	it("puts each library it has into the script that names it", () => {
		const { document, missing } = make(
			"<script src=\"maxhq:lib/chart.js\"></script><script defer src='maxhq:lib/d3.js'></script><p>x</p>"
		);
		expect(missing).toEqual([]);
		expect(document).toContain("<script>window.Chart = 1;</script>");
		// A library's text can't end the script early.
		expect(document).toContain("<script defer>window.d3 = '<\\/script>';</script>");
		expect(document).not.toContain("/visual-lib/");
	});

	it("names each library it doesn't have, once", () => {
		const { document, missing } = make(
			'<script src="maxhq:lib/three.js"></script><script src="maxhq:lib/chart.js"></script><script type="module">import "maxhq:lib/three.js";</script>'
		);
		expect(missing).toEqual(["maxhq:lib/three.js"]);
		expect(document).toContain('<script src="/visual-lib/three.js"></script>');
		expect(document).toContain("<script>window.Chart = 1;</script>");
	});

	it("opens only web and mail links outside the app", () => {
		expect(opensFromVisual("https://example.com/a")).toBe(true);
		expect(opensFromVisual("http://example.com")).toBe(true);
		expect(opensFromVisual("mailto:someone@example.com")).toBe(true);
		expect(opensFromVisual("javascript:alert(1)")).toBe(false);
		expect(opensFromVisual("file:///etc/hosts")).toBe(false);
		expect(opensFromVisual("tel:123")).toBe(false);
		expect(opensFromVisual("not a url")).toBe(false);
		expect(opensFromVisual("HTTPS://Example.com")).toBe(true);
		expect(opensFromVisual("https://")).toBe(false);
		expect(opensFromVisual("mailto:")).toBe(false);
	});

	it("reads what the bridge carries, and nothing it can't", () => {
		expect(readBridgeReport('{"type":"maxhq-visual:link","url":"https://a.b"}')).toEqual({
			type: "maxhq-visual:link",
			url: "https://a.b",
		});
		expect(readBridgeReport("{nope")).toBeNull();
	});
});
