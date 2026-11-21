// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { openDeviceTab } from "@/test/settings";
import type { DeviceId } from "@maxhq/shared/bindings";
import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

/** Opens the Phones page, with network access on for `networkOn`. */
const openPhones = async (networkOn?: DeviceId) => {
	const source = mockSource({ now: () => NOW, tick: false, scenario: "connected" });
	if (networkOn) {
		await source.setDeviceSettings({
			device: networkOn,
			default_permission: "auto",
			workspace_network: null,
			workspace_pool: null,
			auto_settle_days: 5,
			snooze_at_limit: false,
			continue_claude_at_limit: false,
			continue_codex_at_limit: false,
			network: { on: true },
		});
	}
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={createUiStore()} />
		</FixedNow>
	);
	await screen.findAllByRole("button", { name: /devbox/ });
	const dialog = await openDeviceTab("Phones");
	return {
		source,
		add: within(await within(dialog).findByRole("region", { name: "Add phone" })),
		phones: within(await within(dialog).findByRole("region", { name: "Phones" })),
	};
};

describe("Phones settings", () => {
	it("ticks the devices the phone can reach and shows the code through one", async () => {
		const { source, add } = await openPhones();
		const begin = vi.spyOn(source, "beginPhonePair");
		expect(add.getByRole("checkbox", { name: "devbox" })).toBeChecked();
		expect(add.getByRole("checkbox", { name: "MacBook Pro" })).not.toBeChecked();
		// Both devices are connected, and the line below says why the phone can't reach one.
		expect(add.queryByText("Not connected")).not.toBeInTheDocument();
		expect(
			add.getByText("MacBook Pro: Turn on Network access on its tab to pair a phone with it.")
		).toBeInTheDocument();

		fireEvent.click(add.getByRole("button", { name: "Show the code" }));
		expect(await add.findByRole("img", { name: "Pairing code" })).toBeInTheDocument();
		expect(begin).toHaveBeenCalledExactlyOnceWith(DEVBOX, []);
		expect(add.getByText(/It works once, for 10 min more/)).toBeInTheDocument();
	});

	it("pairs through a device the phone reaches only over the network", async () => {
		const { source, add } = await openPhones(MAC);
		const begin = vi.spyOn(source, "beginPhonePair");
		// It starts ticked, as a device the phone can reach does.
		expect(add.getByRole("checkbox", { name: "MacBook Pro" })).toBeChecked();
		expect(add.queryByText(/MacBook Pro:/)).not.toBeInTheDocument();
		fireEvent.click(add.getByRole("checkbox", { name: "devbox" }));

		fireEvent.click(add.getByRole("button", { name: "Show the code" }));
		expect(await add.findByRole("img", { name: "Pairing code" })).toBeInTheDocument();
		expect(begin).toHaveBeenCalledExactlyOnceWith(MAC, []);
	});

	it("asks before a phone may do everything, and stops it at once", async () => {
		const { source, phones } = await openPhones();
		const set = vi.spyOn(source, "setPhoneEverything");
		const toggle = phones.getByRole("switch", { name: "Let iPhone do everything" });
		expect(toggle).not.toBeChecked();
		const card = "Let iPhone do everything?";

		// A no keeps it off and sends nothing.
		fireEvent.click(toggle);
		const asked = within(await phones.findByRole("region", { name: card }));
		expect(asked.getByText(/Anyone holding this phone could start agents/)).toBeInTheDocument();
		fireEvent.click(asked.getByRole("button", { name: "Keep it off" }));
		expect(phones.queryByRole("region", { name: card })).not.toBeInTheDocument();
		expect(toggle).not.toBeChecked();
		expect(set).not.toHaveBeenCalled();

		fireEvent.click(toggle);
		fireEvent.click(
			within(await phones.findByRole("region", { name: card })).getByRole("button", {
				name: "Let it do everything",
			})
		);
		expect(set).toHaveBeenCalledExactlyOnceWith("phone-sample", true);
		expect(await phones.findByText("Can do everything on devbox")).toBeInTheDocument();
		expect(phones.getByRole("switch", { name: "Let iPhone do everything" })).toBeChecked();

		// Off needs no card.
		set.mockClear();
		fireEvent.click(phones.getByRole("switch", { name: "Let iPhone do everything" }));
		expect(set).toHaveBeenCalledExactlyOnceWith("phone-sample", false);
		expect(phones.queryByRole("region", { name: card })).not.toBeInTheDocument();
		await waitFor(() =>
			expect(
				phones.getByRole("switch", { name: "Let iPhone do everything" })
			).not.toBeChecked()
		);
	});

	it("names the devices the switch didn't reach", async () => {
		const { source, phones } = await openPhones();
		vi.spyOn(source, "setPhoneEverything").mockResolvedValue({
			status: "ok",
			data: ["ws-alpha"],
		});
		fireEvent.click(phones.getByRole("switch", { name: "Let iPhone do everything" }));
		fireEvent.click(await phones.findByRole("button", { name: "Let it do everything" }));
		expect(
			await phones.findByText(
				"Couldn't reach ws-alpha. The phone can do everything there once it's connected again."
			)
		).toBeInTheDocument();
	});

	it("makes a new code for the devices the phone hasn't signed in to", async () => {
		// Over SSH alone the phone needs no code, so nothing shows.
		expect((await openPhones()).phones.queryByText(/Not signed in on/)).not.toBeInTheDocument();
		cleanup();

		const { source, phones } = await openPhones(DEVBOX);
		const renew = vi.spyOn(source, "renewPhoneCodes");
		expect(phones.getByText("Not signed in on devbox")).toBeInTheDocument();
		expect(phones.getByText("A new code works for 10 minutes.")).toBeInTheDocument();
		fireEvent.click(phones.getByRole("button", { name: "New code for iPhone" }));
		expect(
			await phones.findByText("Open MaxHQ on the phone within 10 minutes.")
		).toBeInTheDocument();
		expect(renew).toHaveBeenCalledExactlyOnceWith("phone-sample");

		renew.mockResolvedValue({ status: "ok", data: ["devbox"] });
		fireEvent.click(phones.getByRole("button", { name: "New code for iPhone" }));
		expect(
			await phones.findByText("Couldn't reach devbox. Try again once it's connected.")
		).toBeInTheDocument();
	});

	it("saves the address a phone dials a device at from the device's settings", async () => {
		const source = mockSource({ now: () => NOW, tick: false, scenario: "connected" });
		const save = vi.spyOn(source, "setPhoneAddress");
		const ui = createUiStore();
		ui.getState().setMode("devices");
		ui.getState().selectLink("link-devbox");
		render(
			<FixedNow value={NOW}>
				<App source={source} ui={ui} />
			</FixedNow>
		);
		fireEvent.click(await screen.findByRole("button", { name: "Settings for devbox" }));
		const dialog = await screen.findByRole("dialog", { name: "Settings" });
		const phones = within(within(dialog).getByRole("region", { name: "Connection" }));
		await phones.findByRole("textbox", { name: "Address for the phone" });
		const field = phones.getByRole("textbox", { name: "Address for the phone" });
		fireEvent.change(field, { target: { value: " devbox.example.net:2200 " } });
		fireEvent.keyDown(field, { key: "Enter" });
		expect(await phones.findByText("Saved")).toBeInTheDocument();
		expect(save).toHaveBeenCalledExactlyOnceWith(DEVBOX, "devbox.example.net:2200");
		expect(field).toHaveValue("devbox.example.net:2200");
	});
});
