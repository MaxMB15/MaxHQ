// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

// The mock source before any device is set up.
const noDevices = (): DataSource => {
	const mock = mockSource({ now: () => NOW, tick: false });
	return {
		...mock,
		snapshot: async () => {
			const snapshot = await mock.snapshot();
			const entities = snapshot.entities.filter((e) => e.entity !== "device_link");
			return { ...snapshot, entities };
		},
	};
};

describe("the devices view with no devices", () => {
	it("offers to add one", async () => {
		const ui = createUiStore();
		ui.getState().setMode("devices");
		render(
			<FixedNow value={NOW}>
				<App source={noDevices()} ui={ui} />
			</FixedNow>
		);
		const main = screen.getByRole("main");
		expect(await within(main).findByText("No devices yet.")).toBeInTheDocument();
		fireEvent.click(within(main).getByRole("button", { name: "Add device" }));
		expect(await screen.findByRole("dialog", { name: "Add a device" })).toBeInTheDocument();
	});
});
