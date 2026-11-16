// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { beforeEach, describe, expect, it } from "vitest";

import { clearKept } from "@maxhq/shared/agent-view/kept";
import type { AgentId } from "@maxhq/shared/bindings";

import {
	cardCap,
	cardHeight,
	FIRST_HEIGHT,
	heightKey,
	keepHeight,
	knownHeight,
	sameVisual,
} from "./card";
import type { VisualToOpen } from "./visual";

const AGENT = "agent-1" as AgentId;
const PAGE: VisualToOpen = { kind: "page", title: "Usage", html: "<p>hi</p>" };
const DEVICE: VisualToOpen = {
	kind: "device",
	title: "Usage",
	agent: AGENT,
	path: "/a/usage.html",
};

beforeEach(() => clearKept());

describe("a visual's card", () => {
	it("takes most of the screen at most, and never less than its first height", () => {
		expect(cardCap(800)).toBe(560);
		expect(cardCap(100)).toBe(FIRST_HEIGHT);
	});

	it("draws as tall as its page, and scrolls inside past the cap", () => {
		expect(cardHeight(300, 560)).toEqual({ height: 300, scrolls: false });
		expect(cardHeight(560, 560)).toEqual({ height: 560, scrolls: false });
		expect(cardHeight(2000, 560)).toEqual({ height: 560, scrolls: true });
	});

	it("starts at the height its page last drew, by page or by file", () => {
		expect(knownHeight(heightKey(PAGE))).toBe(FIRST_HEIGHT);
		keepHeight(heightKey(PAGE), 420);
		expect(knownHeight(heightKey({ ...PAGE, title: "Other" }))).toBe(420);
		expect(knownHeight(heightKey({ ...PAGE, html: "<p>hey</p>" }))).toBe(FIRST_HEIGHT);
		keepHeight(heightKey(DEVICE), 240);
		expect(knownHeight(heightKey({ ...DEVICE }))).toBe(240);
		expect(knownHeight(heightKey({ ...DEVICE, path: "/a/other.html" }))).toBe(FIRST_HEIGHT);
	});

	it("knows a visual drawn again as a new object", () => {
		expect(sameVisual(PAGE, { ...PAGE })).toBe(true);
		expect(sameVisual(DEVICE, { ...DEVICE })).toBe(true);
		expect(sameVisual(PAGE, { ...PAGE, html: "<p>hey</p>" })).toBe(false);
		expect(sameVisual(PAGE, { ...PAGE, title: "Other" })).toBe(false);
		expect(sameVisual(DEVICE, { ...DEVICE, path: "/a/other.html" })).toBe(false);
		expect(sameVisual(PAGE, DEVICE)).toBe(false);
	});
});
