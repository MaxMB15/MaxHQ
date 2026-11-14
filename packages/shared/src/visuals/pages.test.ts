// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it, vi } from "vitest";

import type { AttachmentRef } from "../bindings";
import { createVisualPages } from "./pages";

const file = (id: string): AttachmentRef => ({
	id,
	name: "visual.html",
	kind: "file",
	bytes: 10,
	mime: "text/html",
	path: `/staged/${id}.html`,
	thumbnail: null,
});

describe("createVisualPages", () => {
	it("reads a page once and tells each listener", async () => {
		const read = vi.fn(async () => "<p>hi</p>");
		const pages = createVisualPages(read);
		const heard = vi.fn();
		const undo = pages.subscribe(heard);
		expect(pages.get("a")).toBeUndefined();
		expect(pages.load("agent-1", file("a"))).toEqual({ status: "loading" });
		expect(pages.load("agent-1", file("a"))).toEqual({ status: "loading" });
		await vi.waitFor(() =>
			expect(pages.get("a")).toEqual({ status: "ready", html: "<p>hi</p>" })
		);
		expect(read).toHaveBeenCalledTimes(1);
		expect(heard).toHaveBeenCalledTimes(2);
		undo();
		pages.put("b", "x");
		expect(heard).toHaveBeenCalledTimes(2);
	});

	it("keeps a failure until it's tried again", async () => {
		const read = vi
			.fn<() => Promise<string>>()
			.mockRejectedValueOnce(new Error("The file is gone"))
			.mockResolvedValueOnce("<p>back</p>");
		const pages = createVisualPages(read);
		pages.load("agent-1", file("a"));
		await vi.waitFor(() =>
			expect(pages.get("a")).toEqual({ status: "failed", error: "The file is gone" })
		);
		expect(pages.load("agent-1", file("a")).status).toBe("failed");
		expect(pages.retry("agent-1", file("a"))).toEqual({ status: "loading" });
		await vi.waitFor(() =>
			expect(pages.get("a")).toEqual({ status: "ready", html: "<p>back</p>" })
		);
		expect(pages.retry("agent-1", file("a")).status).toBe("ready");
		expect(read).toHaveBeenCalledTimes(2);
	});

	it("keeps a staged page and drops the oldest past the bound", () => {
		const pages = createVisualPages(async () => "", 2);
		pages.put("a", "1");
		pages.put("b", "2");
		pages.put("c", "3");
		expect(pages.get("a")).toBeUndefined();
		expect(pages.get("c")).toEqual({ status: "ready", html: "3" });
	});
});
