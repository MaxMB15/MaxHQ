// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { PREFS_KEY, readPrefs } from "./prefs";

const onMac = () =>
	vi
		.spyOn(navigator, "userAgent", "get")
		.mockReturnValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");

const renderApp = () =>
	render(
		<FixedNow value={NOW}>
			<App source={mockSource({ now: () => NOW, tick: false })} ui={createUiStore()} />
		</FixedNow>
	);

const sidebar = () => screen.queryByRole("navigation", { name: "Agents" });
const edge = () => screen.getByRole("separator", { name: "Resize the sidebar" });
const saved = () => readPrefs(localStorage.getItem(PREFS_KEY));

const commandB = () =>
	act(() => {
		fireEvent.keyDown(document.body, { code: "KeyB", key: "b", metaKey: true });
	});

const drag = (from: number, ...to: number[]) => {
	fireEvent.pointerDown(edge(), { button: 0, pointerId: 1, clientX: from });
	for (const x of to) fireEvent.pointerMove(window, { pointerId: 1, clientX: x });
	fireEvent.pointerUp(window, { pointerId: 1, clientX: to.at(-1) });
};

afterEach(() => {
	vi.restoreAllMocks();
});

describe("hiding the sidebar", () => {
	it("hides and shows on ⌘B, and the header's button brings it back", async () => {
		onMac();
		renderApp();
		await within(sidebar()!).findByRole("region", { name: "Mosaic" });
		const hide = screen.getByRole("button", { name: "Hide the sidebar" });
		expect(hide).toHaveAttribute("title", "Hide the sidebar ⌘B");
		expect(hide).toHaveAttribute("aria-keyshortcuts", "Meta+B");

		commandB();
		expect(sidebar()).toBeNull();
		expect(screen.getByRole("button", { name: "Show the sidebar" })).toHaveAttribute(
			"title",
			"Show the sidebar ⌘B"
		);
		commandB();
		expect(sidebar()).not.toBeNull();

		commandB();
		fireEvent.click(screen.getByRole("button", { name: "Show the sidebar" }));
		expect(sidebar()).not.toBeNull();
	});

	it("moves focus from inside the sidebar to the button that brings it back", async () => {
		onMac();
		renderApp();
		const field = await screen.findByRole("searchbox", { name: "Filter agents" });
		field.focus();
		commandB();
		expect(screen.getByRole("button", { name: "Show the sidebar" })).toHaveFocus();
	});

	it("stays hidden when the window draws again from its saved settings", async () => {
		onMac();
		const first = renderApp();
		await within(sidebar()!).findByRole("region", { name: "Mosaic" });
		commandB();
		expect(saved().open).toBe(false);
		first.unmount();

		renderApp();
		await screen.findByRole("button", { name: "Show the sidebar" });
		expect(sidebar()).toBeNull();
		fireEvent.click(screen.getByRole("button", { name: "Show the sidebar" }));
		expect(sidebar()).not.toBeNull();
		expect(saved().open).toBe(true);
	});
});

describe("resizing the sidebar", () => {
	it("drags its edge between the narrowest and the widest, and saves on letting go", async () => {
		renderApp();
		await within(sidebar()!).findByRole("region", { name: "Mosaic" });
		expect(edge()).toHaveAttribute("aria-valuenow", "288");
		expect(edge()).toHaveAttribute("aria-valuemin", "220");
		expect(edge()).toHaveAttribute("aria-valuemax", "480");
		expect(edge()).toHaveClass("cursor-col-resize");

		drag(288, 320, 100);
		expect(edge()).toHaveAttribute("aria-valuenow", "220");
		expect(edge().parentElement).toHaveStyle({ width: "220px" });
		expect(saved().width).toBe(220);

		drag(220, 9000);
		expect(edge()).toHaveAttribute("aria-valuenow", "480");
		expect(saved().width).toBe(480);
		// The header's start keeps the box lined up with the main area.
		const start = screen.getByRole("button", { name: "Hide the sidebar" }).parentElement;
		expect(start).toHaveStyle({ width: "466px" });
		expect(document.body.style.cursor).toBe("");
	});

	it("puts the default width back on a double-click", async () => {
		renderApp();
		await within(sidebar()!).findByRole("region", { name: "Mosaic" });
		drag(288, 400);
		expect(edge()).toHaveAttribute("aria-valuenow", "400");
		fireEvent.doubleClick(edge());
		expect(edge()).toHaveAttribute("aria-valuenow", "288");
		expect(saved().width).toBe(288);
	});

	it("moves its edge with the arrow keys", async () => {
		renderApp();
		await within(sidebar()!).findByRole("region", { name: "Mosaic" });
		edge().focus();
		expect(edge()).toHaveFocus();
		fireEvent.keyDown(edge(), { key: "ArrowLeft" });
		expect(edge()).toHaveAttribute("aria-valuenow", "272");
		fireEvent.keyDown(edge(), { key: "ArrowRight" });
		fireEvent.keyDown(edge(), { key: "ArrowRight" });
		expect(edge()).toHaveAttribute("aria-valuenow", "304");
		expect(saved().width).toBe(304);
	});

	it("starts at the width it was left at", async () => {
		localStorage.setItem(PREFS_KEY, JSON.stringify({ version: 1, width: 360 }));
		renderApp();
		await within(sidebar()!).findByRole("region", { name: "Mosaic" });
		expect(edge()).toHaveAttribute("aria-valuenow", "360");
	});
});
