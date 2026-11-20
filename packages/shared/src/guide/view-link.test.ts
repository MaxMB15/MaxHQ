// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { isViewLink, parseViewLink, VIEWS } from "./view-link";

describe("a view link", () => {
	it("names each view the app opens", () => {
		expect(parseViewLink("maxhq://settings")).toEqual({ kind: "settings", page: null });
		expect(parseViewLink("maxhq://settings/editor/")).toEqual({
			kind: "settings",
			page: "editor",
		});
		expect(parseViewLink("maxhq://agent/a%201")).toEqual({ kind: "agent", agent: "a 1" });
		expect(parseViewLink("maxhq://project/p1")).toEqual({
			kind: "project",
			project: "p1",
			tab: "general",
		});
		expect(parseViewLink("maxhq://project/p1/pulls")).toEqual({
			kind: "project",
			project: "p1",
			tab: "pulls",
		});
		expect(parseViewLink("maxhq://note/p1/n1")).toEqual({
			kind: "note",
			project: "p1",
			note: "n1",
		});
		expect(parseViewLink("maxhq://device/mac/memory")).toEqual({
			kind: "device",
			device: "mac",
			card: "memory",
		});
		expect(parseViewLink("maxhq://view/devices")).toEqual({ kind: "mode", mode: "devices" });
	});

	it("is null when it isn't one the app knows", () => {
		for (const href of [
			"https://example.com",
			"maxhq://",
			"maxhq://agent",
			"maxhq://agent/a1/more",
			"maxhq://project/p1/files",
			"maxhq://note/p1",
			"maxhq://view/notes",
			"maxhq://settings//editor",
			"maxhq://agent/%E0%A4%A",
			"maxhq://unknown/x",
		]) {
			expect(parseViewLink(href), href).toBeNull();
		}
		expect(isViewLink(undefined)).toBe(false);
	});

	it("reads every view the guide's map lists, braces filled", () => {
		for (const view of VIEWS) {
			const filled = view.link.replace(/\{\w+\}/g, "x");
			expect(parseViewLink(filled), view.id).not.toBeNull();
		}
	});
});
