// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import type { InvokeArgs } from "@tauri-apps/api/core";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { type Services, ServicesProvider, useServices } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { noticesOf } from "@/sidebar/notices";
import { createUiStore } from "@maxhq/shared/ui/store";

import { ScreenToggle } from "./screen-toggle";

const app = globalThis as { isTauri?: boolean };

/** The pretend app: what it holds, the calls made, and why a change fails. */
const fake = {
	on: false as boolean | null,
	calls: [] as [string, InvokeArgs | undefined][],
	fails: null as string | null,
};

const fakeApp = (on: boolean | null) => {
	app.isTauri = true;
	fake.on = on;
	fake.calls = [];
	fake.fails = null;
	mockIPC((command, args) => {
		fake.calls.push([command, args]);
		switch (command) {
			case "screen_on":
				return fake.on;
			case "set_screen_on":
				if (fake.fails) throw { kind: "failed", message: fake.fails };
				fake.on = (args as { on: boolean }).on;
				return null;
		}
	});
};

afterEach(() => {
	clearMocks();
	delete app.isTauri;
});

const renderSun = async (on: boolean | null) => {
	fakeApp(on);
	let services: Services | undefined;
	const Grab = () => {
		services = useServices();
		return null;
	};
	await act(async () => {
		render(
			<ServicesProvider source={mockSource({ tick: false })} ui={createUiStore()}>
				<Grab />
				<ScreenToggle />
			</ServicesProvider>
		);
	});
	return services!;
};

const sun = () => screen.getByRole("button", { name: "Keep the screen on" });

const click = (button: HTMLElement) =>
	act(async () => {
		fireEvent.click(button);
		await Promise.resolve();
	});

describe("the header's sun", () => {
	it("shows whether the screen stays on, and flips it", async () => {
		await renderSun(false);
		expect(sun()).toHaveAttribute("aria-pressed", "false");
		expect(sun()).toHaveAttribute(
			"title",
			"Keep the screen on, so it never dims and the Mac never sleeps, on battery too"
		);

		await click(sun());
		expect(sun()).toHaveAttribute("aria-pressed", "true");
		expect(sun()).toHaveAttribute(
			"title",
			"The screen stays on. Click to let it dim and sleep as your settings say"
		);

		await click(sun());
		expect(sun()).toHaveAttribute("aria-pressed", "false");
		expect(fake.calls.filter(([name]) => name === "set_screen_on")).toEqual([
			["set_screen_on", { on: true }],
			["set_screen_on", { on: false }],
		]);
	});

	it("starts on when the app kept it on", async () => {
		await renderSun(true);
		expect(sun()).toHaveAttribute("aria-pressed", "true");
	});

	it("is left out where the screen can't be kept on", async () => {
		await renderSun(null);
		expect(screen.queryByRole("button")).toBeNull();
	});

	it("goes back and says why when macOS refuses", async () => {
		const services = await renderSun(false);
		fake.fails = "macOS refused the power assertion (code 1)";
		await click(sun());
		expect(sun()).toHaveAttribute("aria-pressed", "false");
		expect(noticesOf(services).store.getState().failures).toEqual([
			"Couldn't keep the screen on: macOS refused the power assertion (code 1)",
		]);
	});
});
