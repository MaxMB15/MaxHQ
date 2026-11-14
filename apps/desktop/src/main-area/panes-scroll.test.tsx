// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A pane in a split must never scroll as a whole. dockview's views scroll and
// its groups hide their overflow, so a label placed outside the flow, a
// sideways swipe or a focus could slide a whole pane aside. The pane theme
// clips them instead, and this checks that rule wins over dockview's own.

import { fireEvent, render, screen, within } from "@testing-library/react";
import dockviewCss from "dockview-react/dist/styles/dockview.css?raw";
import { afterEach, describe, expect, it } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";
import panesCss from "./panes.css?raw";

// Tests load no component CSS, so the two sheets the panes use go in by hand,
// in the order the app loads them.
const SHEETS = [dockviewCss, panesCss];
const added: HTMLStyleElement[] = [];

afterEach(() => {
	for (const sheet of added.splice(0)) sheet.remove();
});

const row = (name: RegExp) =>
	within(screen.getByRole("navigation", { name: "Agents" })).findByRole("option", { name });
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];

describe("pane wrappers", () => {
	it("can't scroll a pane sideways, and keep what a pane places inside it", async () => {
		expect(SHEETS.map((css) => css.length > 1000)).toEqual([true, true]);
		for (const css of SHEETS) {
			const sheet = document.createElement("style");
			sheet.textContent = css;
			document.head.append(sheet);
			added.push(sheet);
		}
		render(
			<FixedNow value={NOW}>
				<App source={mockSource({ now: () => NOW, tick: false })} ui={createUiStore()} />
			</FixedNow>
		);
		fireEvent.click(await row(/M1 observe milestone/));
		fireEvent.click(await row(/Release checklist/));
		fireEvent.click(screen.getAllByRole("button", { name: "Split pane" })[0]!);
		expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(2);

		// The rows of views, the views and the groups clip, even against the
		// inline style dockview gives a group, so nothing can scroll them.
		const wrappers = all(".dv-split-view-container, .dv-view, .dv-groupview");
		expect(wrappers.filter((el) => el.matches(".dv-groupview"))).toHaveLength(2);
		for (const el of wrappers) {
			expect({ el: el.className, overflow: getComputedStyle(el).overflow }).toEqual({
				el: el.className,
				overflow: "clip",
			});
		}

		// What a pane places absolutely, such as a label kept for screen
		// readers, is placed against the pane, not the view around it.
		const contents = all(".dv-content-container");
		expect(contents).toHaveLength(2);
		for (const el of contents) expect(getComputedStyle(el).position).toBe("relative");
	});
});
