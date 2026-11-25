// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)";
const WINDOWS = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

const renderApp = () =>
	render(
		<FixedNow value={NOW}>
			<App source={mockSource({ now: () => NOW, tick: false })} ui={createUiStore()} />
		</FixedNow>
	);

// The title bar, which holds the sidebar's button.
const header = () => screen.getByRole("button", { name: "Hide the sidebar" }).closest("header")!;
const dragged = () => header().querySelectorAll("[data-tauri-drag-region]").length;
const trafficLights = () => header().querySelector(".w-\\[74px\\]");

afterEach(() => {
	vi.restoreAllMocks();
});

describe("the header's window chrome", () => {
	it("leaves room for the traffic lights and drags the window on a Mac", async () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(MAC);
		renderApp();
		await screen.findByRole("button", { name: "Hide the sidebar" });
		expect(header()).toHaveAttribute("data-tauri-drag-region", "");
		expect(dragged()).toBe(4);
		expect(trafficLights()).not.toBeNull();
	});

	it("is a plain toolbar under the system's title bar elsewhere", async () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(WINDOWS);
		renderApp();
		await screen.findByRole("button", { name: "Hide the sidebar" });
		expect(header()).not.toHaveAttribute("data-tauri-drag-region");
		expect(dragged()).toBe(0);
		expect(trafficLights()).toBeNull();
	});
});
