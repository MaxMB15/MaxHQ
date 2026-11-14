// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { onReattach } from "./reattach";

// A group's content box with one pane's content in it, holding a list.
const pane = () => {
	const box = document.createElement("div");
	const part = document.createElement("div");
	part.className = "dv-react-part";
	const list = document.createElement("div");
	part.append(list);
	box.append(part);
	document.body.append(box);
	return { box, part, list };
};

// Lets the observer see the change and the frame after it run.
const frame = () => new Promise((resolve) => requestAnimationFrame(resolve));

afterEach(() => {
	document.body.replaceChildren();
});

describe("onReattach", () => {
	it("calls back once when the content is taken out and put straight back", async () => {
		const { box, part, list } = pane();
		const back = vi.fn();
		const stop = onReattach(list, back);
		part.remove();
		box.append(part);
		part.remove();
		box.append(part);
		await frame();
		expect(back).toHaveBeenCalledTimes(1);
		stop();
	});

	it("waits while the content is out of the page", async () => {
		const { box, part, list } = pane();
		const back = vi.fn();
		const stop = onReattach(list, back);
		part.remove();
		await frame();
		expect(back).not.toHaveBeenCalled();
		box.append(part);
		await frame();
		expect(back).toHaveBeenCalledTimes(1);
		stop();
	});

	it("follows the content into another group", async () => {
		const { part, list } = pane();
		const other = document.createElement("div");
		document.body.append(other);
		const back = vi.fn();
		const stop = onReattach(list, back);
		other.append(part);
		await frame();
		part.remove();
		other.append(part);
		await frame();
		expect(back).toHaveBeenCalledTimes(2);
		stop();
	});

	it("ignores other changes beside it and stops when asked", async () => {
		const { box, part, list } = pane();
		const back = vi.fn();
		const stop = onReattach(list, back);
		box.append(document.createElement("div"));
		await frame();
		expect(back).not.toHaveBeenCalled();
		stop();
		part.remove();
		box.append(part);
		await frame();
		expect(back).not.toHaveBeenCalled();
	});

	it("does nothing outside a pane", () => {
		const list = document.createElement("div");
		document.body.append(list);
		expect(() => onReattach(list, vi.fn())()).not.toThrow();
	});
});
