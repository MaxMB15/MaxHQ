// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";

import { type Services, ServicesProvider, useServices } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { CLOUD_SERVERS_SINCE, type DeviceId, type Entity } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { DEVBOX_OPENROUTER_SERVER, OPENROUTER_SERVER } from "@maxhq/shared/data/mock-local-models";
import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { OPENROUTER_URL } from "@maxhq/shared/local-models/openrouter";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { OpenRouterLogin } from "./openrouter-login";
import { AccountsCard } from "./page/accounts";
import { settingsOf } from "./store";

const mock = () => mockSource({ now: () => NOW, tick: false, scenario: "connected", stepMs: 5 });

/** The mock source without the OpenRouter servers of `devices`. */
const without = async (...devices: DeviceId[]) => {
	const source = mock();
	for (const device of devices)
		await source.modelAction(device, {
			action: "remove_server",
			server: device === MAC ? OPENROUTER_SERVER : DEVBOX_OPENROUTER_SERVER,
		});
	return source;
};

// The mock source with `device`'s maxd from before cloud servers.
const beforeSignIn =
	(device: DeviceId) =>
	(source: DataSource): DataSource => ({
		...source,
		snapshot: async () => {
			const snapshot = await source.snapshot();
			const entities = snapshot.entities.map((e): Entity =>
				e.entity === "device_link" && e.device === device
					? { ...e, protocol: CLOUD_SERVERS_SINCE - 1 }
					: e
			);
			return { ...snapshot, entities };
		},
	});

/** `children` with the services they read, and those services. */
const renderWith = (source: DataSource, children: ReactNode) => {
	const held: { services: Services | null } = { services: null };
	const Grab = () => {
		held.services = useServices();
		return null;
	};
	render(
		<FixedNow value={NOW}>
			<ServicesProvider source={source} ui={createUiStore()}>
				<Grab />
				{children}
			</ServicesProvider>
		</FixedNow>
	);
	return held;
};

const group = async (name: string) => within(await screen.findByRole("group", { name }));

/** The OpenRouter row of the Logins table, as each device's cell and its button. */
const openRouterRow = async () => {
	const logins = within(await screen.findByRole("region", { name: "Logins" }));
	const heads = logins.getAllByRole("columnheader").map((th) => th.textContent ?? "");
	const row = (await logins.findByRole("rowheader", { name: "OpenRouter" })).closest("tr")!;
	const cells = Array.from(row.querySelectorAll("td"));
	return {
		cells: Object.fromEntries(
			cells.slice(0, -1).map((td, at) => [heads[at + 1], td.textContent])
		),
		button: within(row).getByRole("button"),
	};
};

describe("OpenRouter in the Logins table", () => {
	it("says each device's key with its credits, and manages", async () => {
		const held = renderWith(mock(), <AccountsCard />);
		await waitFor(async () =>
			expect((await openRouterRow()).cells).toEqual({
				Mac: "Signed in · $7.42 left",
				devbox: "Signed in · $4.12 spent",
			})
		);
		const { button } = await openRouterRow();
		expect(button).toHaveTextContent("Manage");
		fireEvent.click(button);
		expect(settingsOf(held.services!).getState().detail).toEqual({
			kind: "login",
			login: "openrouter",
		});
	});

	it("reads a device without a server as not set up", async () => {
		renderWith(await without(DEVBOX), <AccountsCard />);
		await waitFor(async () =>
			expect((await openRouterRow()).cells).toEqual({
				Mac: "Signed in · $7.42 left",
				devbox: "Not set up",
			})
		);
		expect((await openRouterRow()).button).toHaveTextContent("Manage");
	});

	it("offers the Mac to sign in on while no device is", async () => {
		const held = renderWith(await without(MAC, DEVBOX), <AccountsCard />);
		await waitFor(async () =>
			expect((await openRouterRow()).cells).toEqual({
				Mac: "Not set up",
				devbox: "Not set up",
			})
		);
		const { button } = await openRouterRow();
		expect(button).toHaveTextContent("Sign in on Mac");
		fireEvent.click(button);
		expect(settingsOf(held.services!).getState().detail).toEqual({
			kind: "login",
			login: "openrouter",
			device: MAC,
		});
	});
});

describe("the OpenRouter page", () => {
	it("opens on the Mac's server, with its credits, buttons and models", async () => {
		const held = renderWith(mock(), <OpenRouterLogin />);
		expect(
			await screen.findByRole("region", { name: "OpenRouter on Mac" })
		).toBeInTheDocument();
		expect(screen.getByText("Signed in · $7.42 left")).toBeInTheDocument();
		const row = await group("OpenRouter");
		expect(row.getByText(OPENROUTER_URL)).toBeInTheDocument();
		expect(row.getByText(/\$7\.42 left of \$10\.00/)).toBeInTheDocument();
		for (const name of ["Sign in again", "Test", "Remove"])
			expect(row.getByRole("button", { name })).toBeInTheDocument();
		expect(
			row.getByRole("button", { name: "Add admin key for your balance" })
		).toBeInTheDocument();
		// No form: the server is there already.
		expect(screen.queryByRole("button", { name: "Sign in with OpenRouter" })).toBeNull();
		expect(screen.getByText("4 models")).toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Models in Local models" }));
		expect(settingsOf(held.services!).getState().detail).toEqual({
			kind: "local-models",
			device: MAC,
		});
	});

	it("signs a device without a server in, through a round that fails first", async () => {
		renderWith(await without(DEVBOX), <OpenRouterLogin device={DEVBOX} />);
		const page = within(await screen.findByRole("region", { name: "OpenRouter on devbox" }));
		expect(page.getByText("Not set up")).toBeInTheDocument();
		expect(page.getByRole("link", { name: "Get a key" })).toHaveAttribute(
			"href",
			"https://openrouter.ai/settings/keys"
		);
		expect(page.getByLabelText("API key")).toHaveAttribute("type", "password");
		// The form stays: there's no Cancel to close it with.
		expect(page.queryByRole("button", { name: "Cancel" })).toBeNull();
		fireEvent.change(page.getByRole("textbox", { name: "Spending limit" }), {
			target: { value: "25" },
		});
		fireEvent.click(page.getByRole("button", { name: "Sign in with OpenRouter" }));
		expect(await page.findByText("Finish signing in in your browser")).toBeInTheDocument();
		// The sample's first round fails the way an expired code does.
		expect(await page.findByText(/start the sign-in again\.$/)).toHaveClass(
			"text-status-error-text"
		);
		fireEvent.click(page.getByRole("button", { name: "Try again" }));
		// The round added devbox's server, which takes the form's place.
		expect((await group("OpenRouter")).getByText(OPENROUTER_URL)).toBeInTheDocument();
		await waitFor(() =>
			expect(page.queryByRole("button", { name: "Sign in with OpenRouter" })).toBeNull()
		);
		expect(page.getByText(/^Signed in/)).toBeInTheDocument();
	});

	it("opens on the device the picker names", async () => {
		const held = renderWith(mock(), <OpenRouterLogin />);
		await screen.findByRole("region", { name: "OpenRouter on Mac" });
		const picker = screen.getByRole("combobox", { name: /^Device/ });
		fireEvent.click(picker);
		const devbox = (await screen.findAllByRole("option")).find((o) =>
			o.textContent?.includes("devbox")
		)!;
		fireEvent.pointerDown(devbox, { pointerType: "mouse" });
		fireEvent.click(devbox);
		expect(settingsOf(held.services!).getState().detail).toEqual({
			kind: "login",
			login: "openrouter",
			device: DEVBOX,
		});
	});

	it("offers an older maxd a pasted key only", async () => {
		const source = beforeSignIn(MAC)(await without(MAC));
		renderWith(source, <OpenRouterLogin device={MAC} />);
		const page = within(await screen.findByRole("region", { name: "OpenRouter on Mac" }));
		expect(
			await page.findByText("Update this device's maxd to sign in with OpenRouter.")
		).toBeInTheDocument();
		expect(page.queryByRole("button", { name: "Sign in with OpenRouter" })).toBeNull();
		expect(page.queryByRole("textbox", { name: "Spending limit" })).toBeNull();
		expect(page.getByText("Paste an API key")).toBeInTheDocument();
		expect(page.getByLabelText("API key")).toHaveAttribute("type", "password");
	});
});
