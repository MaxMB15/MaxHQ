// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { watchRowFocus } from "./row-focus";

let stop: (() => void) | null = null;

afterEach(() => {
	stop?.();
	stop = null;
	document.body.replaceChildren();
});

const rowFor = (id: string, current = false): HTMLButtonElement => {
	const row = document.createElement("button");
	row.dataset.agentRow = id;
	row.textContent = id;
	if (current) row.setAttribute("aria-current", "true");
	return row;
};

// A list like the sidebar's: rows in two sections with a header button
// between them, and a field.
const list = (current?: string) => {
	const root = document.createElement("div");
	const first = root.appendChild(document.createElement("section"));
	first.append(rowFor("a", current === "a"), rowFor("b", current === "b"));
	root.appendChild(document.createElement("button")).textContent = "Settled";
	const second = root.appendChild(document.createElement("section"));
	second.append(rowFor("c", current === "c"));
	const field = root.appendChild(document.createElement("input"));
	document.body.append(root);
	stop = watchRowFocus(root);
	const row = (id: string) => root.querySelector<HTMLElement>(`[data-agent-row="${id}"]`)!;
	return { root, first, second, field, row };
};

const stops = (root: HTMLElement) =>
	[...root.querySelectorAll<HTMLElement>("[data-agent-row]")]
		.filter((row) => row.tabIndex === 0)
		.map((row) => row.dataset.agentRow);

// The watcher hears of rows that come and go once the DOM settles.
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("the sidebar's rows for the keyboard", () => {
	it("take one Tab stop, on the agent you're looking at", async () => {
		const { root, row } = list("b");
		expect(stops(root)).toEqual(["b"]);

		row("b").removeAttribute("aria-current");
		row("c").setAttribute("aria-current", "true");
		await settle();
		expect(stops(root)).toEqual(["c"]);
	});

	it("take the first row's stop when you look at none of them", () => {
		const { root } = list();
		expect(stops(root)).toEqual(["a"]);
	});

	it("move with Up and Down across the sections, and stop at the ends", () => {
		const { row } = list();
		row("a").focus();
		fireEvent.keyDown(row("a"), { key: "ArrowUp" });
		expect(row("a")).toHaveFocus();
		fireEvent.keyDown(row("a"), { key: "ArrowDown" });
		expect(row("b")).toHaveFocus();
		// Past the header between them.
		fireEvent.keyDown(row("b"), { key: "ArrowDown" });
		expect(row("c")).toHaveFocus();
		fireEvent.keyDown(row("c"), { key: "ArrowDown" });
		expect(row("c")).toHaveFocus();
		fireEvent.keyDown(row("c"), { key: "ArrowUp" });
		expect(row("b")).toHaveFocus();
	});

	it("keep the stop on the row you moved to until focus leaves", async () => {
		const { root, row } = list("a");
		row("a").focus();
		fireEvent.keyDown(row("a"), { key: "ArrowDown" });
		expect(stops(root)).toEqual(["b"]);

		const outside = document.body.appendChild(document.createElement("button"));
		outside.focus();
		expect(stops(root)).toEqual(["a"]);
	});

	it("leave keys with a modifier, from the field, or taken already alone", () => {
		const { field, row } = list();
		row("a").focus();
		for (const mod of ["shiftKey", "altKey", "ctrlKey", "metaKey"]) {
			fireEvent.keyDown(row("a"), { key: "ArrowDown", [mod]: true });
			expect(row("a")).toHaveFocus();
		}
		const taken = new KeyboardEvent("keydown", {
			key: "ArrowDown",
			bubbles: true,
			cancelable: true,
		});
		taken.preventDefault();
		row("a").dispatchEvent(taken);
		expect(row("a")).toHaveFocus();

		field.focus();
		expect(fireEvent.keyDown(field, { key: "ArrowDown" })).toBe(true);
		expect(field).toHaveFocus();
	});

	it("follow the agent to its new row when its row goes", async () => {
		const { root, first, second, row } = list();
		row("b").focus();
		// The agent moves to the other section, as a settle would move it.
		row("b").remove();
		second.append(rowFor("b"));
		await settle();
		expect(row("b")).toHaveFocus();
		expect(stops(root)).toEqual(["b"]);
		expect(first.contains(row("b"))).toBe(false);
	});

	it("land on the row in its place when the agent goes", async () => {
		const { root, row } = list();
		row("b").focus();
		row("b").remove();
		await settle();
		expect(row("c")).toHaveFocus();
		expect(stops(root)).toEqual(["c"]);
	});

	it("leave focus alone when a row goes after you clicked elsewhere", async () => {
		const { row } = list();
		row("b").focus();
		fireEvent.pointerDown(document.body);
		row("b").blur();
		row("b").remove();
		await settle();
		expect(document.body).toHaveFocus();
	});

	it("stop listening once stopped", () => {
		const { row } = list();
		stop?.();
		stop = null;
		row("a").focus();
		fireEvent.keyDown(row("a"), { key: "ArrowDown" });
		expect(row("a")).toHaveFocus();
	});
});
