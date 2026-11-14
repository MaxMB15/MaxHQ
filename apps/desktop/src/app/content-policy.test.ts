// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import raw from "../../src-tauri/tauri.conf.json?raw";

type Security = { csp: string; devCsp: string };
const conf = JSON.parse(raw) as { app: { security: Security } };

const directives = (csp: string) =>
	new Map(
		csp
			.split(";")
			.map((directive) => directive.trim().split(/\s+/))
			.map(([name, ...sources]) => [name, sources] as const)
	);

const policy = directives(conf.app.security.csp);
const devPolicy = directives(conf.app.security.devCsp);

describe("the webview's content policy", () => {
	// These three keep injected markup from loading a plugin, sending
	// relative links elsewhere or posting a form off the app.
	it.each(["object-src", "base-uri", "form-action"])("shuts off %s", (name) => {
		expect(policy.get(name)).toEqual(["'none'"]);
	});

	// The file viewer's frame is the only page the app frames.
	it("frames only the file viewer", () => {
		expect(policy.get("frame-src")).toEqual(["maxview:", "http://maxview.localhost"]);
	});

	it("is the same in development, where the viewer frame comes from the dev server", () => {
		expect(devPolicy.get("frame-src")).toEqual([
			...policy.get("frame-src")!,
			"http://localhost:1420",
		]);
		const rest = (map: typeof policy) => [...map].filter(([name]) => name !== "frame-src");
		expect(rest(devPolicy)).toEqual(rest(policy));
	});

	it("doesn't allow the asset scheme, which the app never turns on", () => {
		expect(conf.app.security.csp).not.toContain("asset");
	});

	it("still loads the media the app serves itself", () => {
		expect(policy.get("img-src")).toContain("maxmedia:");
		expect(policy.get("media-src")).toContain("maxmedia:");
		// The file pane reads a file's bytes to hand them to the viewer.
		expect(policy.get("connect-src")).toContain("maxmedia:");
	});
});
