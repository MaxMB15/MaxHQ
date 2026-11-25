// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { Entity } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { AppNoSandboxNote, NO_SANDBOX, NoSandboxNote, runsWithoutSandbox } from "./no-sandbox-note";

const WINDOWS =
	"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";

// The mock source with devbox on Windows.
const devboxOnWindows = (source: DataSource): DataSource => ({
	...source,
	snapshot: async () => {
		const snapshot = await source.snapshot();
		const entities = snapshot.entities.map((e): Entity =>
			e.entity === "device" && e.id === "dev-devbox" ? { ...e, os: "windows" } : e
		);
		return { ...snapshot, entities };
	},
});

// Settings, opened from the device's page for `link`.
const openSettings = async (link: string, wrap = (s: DataSource) => s) => {
	const source = wrap(mockSource({ now: () => NOW, tick: false, scenario: "connected" }));
	const ui = createUiStore();
	ui.getState().setMode("devices");
	ui.getState().selectLink(link);
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	fireEvent.click(await screen.findByRole("button", { name: /^Settings for / }));
	return screen.findByRole("dialog", { name: "Settings" });
};

const newAgents = (name: string) => screen.findByRole("region", { name: `Settings for ${name}` });

afterEach(() => {
	vi.restoreAllMocks();
});

describe("the no-sandbox note", () => {
	it("is for Windows devices only", () => {
		expect(runsWithoutSandbox("windows")).toBe(true);
		for (const os of ["macos", "linux", "freebsd", null, undefined, ""]) {
			expect(runsWithoutSandbox(os)).toBe(false);
		}
		const { container, rerender } = render(<NoSandboxNote os="windows" />);
		expect(container).toHaveTextContent(NO_SANDBOX);
		rerender(<NoSandboxNote os="linux" />);
		expect(container).toBeEmptyDOMElement();
	});

	it("shows in Settings only when the app runs on Windows", () => {
		const { container, rerender } = render(<AppNoSandboxNote />);
		expect(container).toBeEmptyDOMElement();
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(WINDOWS);
		rerender(<AppNoSandboxNote />);
		expect(container).toHaveTextContent(NO_SANDBOX);
	});

	it("shows on a Windows device's New agents card, and not on a Mac's", async () => {
		await openSettings("link-devbox", devboxOnWindows);
		expect(within(await newAgents("devbox")).getByText(NO_SANDBOX)).toBeInTheDocument();
		// The app runs on a Mac here, so the Permissions group says nothing.
		const settings = screen.getByRole("dialog", { name: "Settings" });
		expect(within(settings).getAllByText(NO_SANDBOX)).toHaveLength(1);
	});

	it("doesn't show for a device off Windows", async () => {
		await openSettings("link-devbox");
		await newAgents("devbox");
		expect(screen.queryByText(NO_SANDBOX)).toBeNull();
	});

	it("shows once above the permissions when the app runs on Windows", async () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(WINDOWS);
		// The taskbar overlay draws on a canvas, which the test's page lacks.
		vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
		const settings = await openSettings("link-devbox");
		await newAgents("devbox");
		const permissions = within(settings).getByRole("region", { name: "Permissions" });
		expect(within(permissions).getAllByText(NO_SANDBOX)).toHaveLength(1);
	});
});
