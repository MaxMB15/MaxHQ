// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const onMac = () =>
	vi
		.spyOn(navigator, "userAgent", "get")
		.mockReturnValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");

const slash = (target: Element) =>
	act(() => {
		fireEvent.keyDown(target, { code: "Slash", key: "/", metaKey: true });
	});

afterEach(() => {
	vi.restoreAllMocks();
});

describe("the keyboard shortcuts sheet", () => {
	it("opens on Command slash, lists the keys by group, and closes on it again", async () => {
		onMac();
		render(
			<FixedNow value={NOW}>
				<App source={mockSource({ now: () => NOW, tick: false })} ui={createUiStore()} />
			</FixedNow>
		);
		await screen.findAllByRole("navigation");
		slash(document.body);
		const sheet = await screen.findByRole("dialog", { name: "Keyboard shortcuts" });

		const panes = within(sheet).getByRole("region", { name: "Panes" });
		expect(within(panes).getByText("Split down")).toBeInTheDocument();
		expect(within(panes).getByText("⇧⌘D")).toBeInTheDocument();
		// Reopening works in either mode, so only it carries no mark of its own.
		expect(within(panes).getAllByText("Projects only")).toHaveLength(
			within(panes).getAllByRole("listitem").length - 1
		);
		const composer = within(sheet).getByRole("region", { name: "Composer" });
		expect(within(composer).getByText("⌘U")).toBeInTheDocument();
		expect(within(composer).queryByText("Projects only")).toBeNull();
		const sidebar = within(sheet).getByRole("region", { name: "Sidebar" });
		expect(within(sidebar).getByText("⌘B")).toBeInTheDocument();
		expect(within(sidebar).getByText("⌥⌘F")).toBeInTheDocument();
		const find = within(sheet).getByRole("region", { name: "Find" });
		// A group that works only in projects mode says so once, by its name.
		expect(within(find).getAllByText("Projects only")).toHaveLength(1);

		slash(sheet);
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
	});
});
