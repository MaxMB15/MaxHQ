// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import type { SetupHost } from "@maxhq/shared/fleet/wire";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { follow, phaseText, START } from "./add-device";

// Each step of the sample setup takes a millisecond, not the sample build's
// 700, so a wait only lasts as long as the renders. Several steps can still
// add up on a busy machine.
const SLOW = { timeout: 5000 };

const open = async (hosts?: SetupHost[]) => {
	const source = mockSource({ now: () => NOW, tick: false, stepMs: 1 });
	if (hosts) vi.spyOn(source, "listSetupHosts").mockResolvedValue(hosts);
	const ui = createUiStore();
	ui.getState().setMode("devices");
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	fireEvent.click(await screen.findByRole("button", { name: "Add device" }));
	return { source, ui, dialog: await screen.findByRole("dialog", { name: "Add a device" }) };
};

const pick = async (dialog: HTMLElement, host: string) => {
	fireEvent.click(await within(dialog).findByRole("radio", { name: new RegExp(`^${host}`) }));
	fireEvent.click(within(dialog).getByRole("button", { name: "Next" }));
};

describe("the Add device wizard", () => {
	it("lists the hosts and marks the one set up already", async () => {
		const { dialog } = await open();
		expect(await within(dialog).findByRole("radio", { name: /^devbox/ })).toBeDisabled();
		const spare = within(dialog).getByRole("radio", { name: /^spare-box/ });
		expect(spare).toHaveTextContent("On your tailnet, spare-box.tail1234.ts.net");
		expect(within(dialog).getByRole("button", { name: "Next" })).toBeDisabled();
		fireEvent.click(spare);
		expect(spare).toHaveAttribute("aria-checked", "true");
	});

	it("waits for you to trust the host key, then sets the device up", async () => {
		const { source, ui, dialog } = await open();
		const setup = vi.spyOn(source, "setupDevice");
		const confirm = vi.spyOn(source, "confirmHostKey");
		await pick(dialog, "spare-box");
		fireEvent.change(await screen.findByLabelText("Claude token"), {
			target: { value: "sk-ant-oat01-sample" },
		});
		fireEvent.click(screen.getByRole("button", { name: "Set up spare-box" }));
		expect(setup).toHaveBeenCalledWith(
			"spare-box",
			[expect.objectContaining({ secret: "claude_token", token: "sk-ant-oat01-sample" })],
			expect.any(Function)
		);

		const key = await screen.findByRole("group", { name: "Host key" }, SLOW);
		expect(key).toHaveTextContent("SHA256:q3Vt0bYx9mJ2kZ8hR4fWc1nP7sLd6uEa5oGi0XyTz4M");
		expect(key).toHaveTextContent(
			"spare-box shows this ED25519 key. Check it matches the one the machine prints for ssh-keygen -lf on its host key."
		);
		expect(screen.queryByText("Checking the system")).toBeNull();
		fireEvent.click(within(key).getByRole("button", { name: "Trust this key" }));
		expect(confirm).toHaveBeenCalledWith("setup-1", true);

		expect(
			await screen.findByRole("heading", { name: "spare-box is set up" }, SLOW)
		).toBeVisible();
		const steps = within(screen.getByRole("list", { name: "Steps" })).getAllByRole("listitem");
		// The app sends each step's phase, and the list words it.
		expect(steps.map((s) => s.textContent)).toEqual([
			"Reading spare-box's host key",
			"Copying maxd",
			"Added MaxHQ's key",
			"Connecting to maxd",
			"Sending the logins you picked",
		]);
		fireEvent.click(screen.getByRole("button", { name: "Show spare-box" }));
		expect(ui.getState().selectedLink).toBe("link-spare-box");
	});

	it("runs nothing when you don't trust the key", async () => {
		const { dialog } = await open();
		await pick(dialog, "spare-box");
		fireEvent.click(await screen.findByRole("button", { name: "Set up spare-box" }));
		const key = await screen.findByRole("group", { name: "Host key" }, SLOW);
		fireEvent.click(within(key).getByRole("button", { name: "Don't trust" }));
		expect(
			await screen.findByText("You didn't trust the host key, so nothing ran")
		).toBeVisible();
		expect(screen.getByRole("button", { name: "Try again" })).toBeVisible();
	});

	it("offers a Windows device's WSL distros by name, and not one on WSL 1", async () => {
		const distro = (name: string, version: number, isDefault: boolean): SetupHost => ({
			host: `wsl:${name}@dev@pc`,
			from: "wsl",
			address: null,
			link: null,
			wsl: { distro: name, machine: "Desk PC", here: false, version, default: isDefault },
		});
		const { source, dialog } = await open([
			distro("Ubuntu", 2, true),
			distro("Legacy", 1, false),
		]);
		const setup = vi.spyOn(source, "setupDevice");
		const ubuntu = await within(dialog).findByRole("radio", { name: /^Ubuntu/ });
		expect(ubuntu).toHaveTextContent("WSL on Desk PC, the default distro");
		const legacy = within(dialog).getByRole("radio", { name: /^Legacy/ });
		expect(legacy).toBeDisabled();
		expect(legacy).toHaveTextContent(
			"WSL 1 can't run maxd. Move it to WSL 2 with wsl --set-version Legacy 2."
		);
		await pick(dialog, "Ubuntu");
		expect(await screen.findByRole("heading", { name: "Logins for Ubuntu" })).toBeVisible();
		fireEvent.click(screen.getByRole("button", { name: "Set up Ubuntu" }));
		expect(setup).toHaveBeenCalledWith("wsl:Ubuntu@dev@pc", [], expect.any(Function));
	});

	it("shows the command a host needs to keep maxd running", async () => {
		const { dialog } = await open();
		await pick(dialog, "build-box");
		fireEvent.click(await screen.findByRole("button", { name: "Set up build-box" }));
		const key = await screen.findByRole("group", { name: "Host key" }, SLOW);
		fireEvent.click(within(key).getByRole("button", { name: "Trust this key" }));
		expect(await screen.findByText("loginctl enable-linger $USER", {}, SLOW)).toBeVisible();
		await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("lingering"));
	});
});

describe("follow", () => {
	it("keeps each step once, at the last phase it reached, in the order reached", () => {
		let state = follow(START, {
			type: "step",
			setup: "s",
			step: "host_key",
			phase: "reading_host_key",
		});
		state = follow(state, {
			type: "step",
			setup: "s",
			step: "install",
			phase: "checking_system",
		});
		state = follow(state, { type: "step", setup: "s", step: "install", phase: "copying_maxd" });
		expect(state.steps).toEqual([
			{ step: "host_key", phase: "reading_host_key" },
			{ step: "install", phase: "copying_maxd" },
		]);
		expect(state.setup).toBe("s");
	});

	it("words every phase in the reader's language, the waiting one included", () => {
		expect(phaseText("reading_host_key", "devbox")).toBe("Reading devbox's host key");
		expect(phaseText("waiting_for_answer", "devbox")).toBe(
			"Waiting for your answer about which device this is"
		);
	});
});
