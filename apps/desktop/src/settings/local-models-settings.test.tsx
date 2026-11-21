// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import type { Entity } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { DOWNLOADING_MODEL, MAC_SERVER, OLLAMA_URL } from "@maxhq/shared/data/mock-local-models";
import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { LocalModelsSettingsPage } from "./local-models-settings";

const mock = (wrap = (s: DataSource) => s) =>
	wrap(mockSource({ now: () => NOW, tick: false, scenario: "connected", stepMs: 5 }));

// The mock source with devbox's maxd from before local models.
const oldMaxd = (source: DataSource): DataSource => ({
	...source,
	snapshot: async () => {
		const snapshot = await source.snapshot();
		const entities = snapshot.entities.map((e): Entity =>
			e.entity === "device_link" && e.device === DEVBOX && e.capabilities
				? { ...e, capabilities: { ...e.capabilities, local_models: false } }
				: e
		);
		return { ...snapshot, entities };
	},
});

/** The page alone, with the services it reads. Mounting the whole app is slow. */
const renderPage = (source: DataSource = mock()) => {
	render(
		<FixedNow value={NOW}>
			<ServicesProvider source={source} ui={createUiStore()}>
				<LocalModelsSettingsPage />
			</ServicesProvider>
		</FixedNow>
	);
	return source;
};

const choose = async (combobox: HTMLElement, text: string) => {
	fireEvent.click(combobox);
	const found = (await screen.findAllByRole("option")).find((o) => o.textContent?.includes(text));
	if (!found) throw new Error(`No option "${text}"`);
	fireEvent.pointerDown(found, { pointerType: "mouse" });
	fireEvent.click(found);
};

const showDevbox = async () =>
	choose(await screen.findByRole("combobox", { name: "Device, Mac" }), "devbox");

const group = async (name: string) => within(await screen.findByRole("group", { name }));

describe("the Local models page in the settings", () => {
	it("comes up in the search for llama.cpp, Ollama and GPU", async () => {
		render(
			<FixedNow value={NOW}>
				<App source={mock()} ui={createUiStore()} />
			</FixedNow>
		);
		await screen.findAllByRole("button", { name: /devbox/ });
		fireEvent.click(screen.getByRole("button", { name: "Settings" }));
		const nav = await screen.findByRole("navigation", { name: "Settings pages" });
		const field = within(nav).getByRole("searchbox", { name: "Search settings" });
		for (const words of ["ollama", "llama.cpp", "GPU", "KV cache"]) {
			fireEvent.change(field, { target: { value: words } });
			const app = within(nav).getByRole("group", { name: "App" });
			expect(within(app).getByRole("button", { name: /^Local models/ }), words).toBeVisible();
		}
		fireEvent.click(
			within(within(nav).getByRole("group", { name: "App" })).getByRole("button", {
				name: /^Local models/,
			})
		);
		const dialog = await screen.findByRole("dialog", { name: "Settings" });
		expect(
			await within(dialog).findByRole("region", { name: "Hardware of Mac" })
		).toBeInTheDocument();
	}, 20_000);
});

describe("the Local models page", () => {
	it("shows the Mac's hardware and the GPU limit it leaves alone", async () => {
		renderPage();
		const hardware = within(await screen.findByRole("region", { name: "Hardware of Mac" }));
		expect(hardware.getByText(/32-core GPU/)).toBeInTheDocument();
		expect(hardware.getByText(/MaxHQ doesn't raise the limit/)).toBeInTheDocument();
	});

	it("tags each model with its fit, the fit's meaning as its tooltip", async () => {
		renderPage();
		const row = await group("Qwen3.6-35B-A3B");
		const pill = row.getByText(/^(Fits|Tight|Too large)$/);
		expect(pill.parentElement?.getAttribute("title")).toMatch(/memory/);
		expect(row.getByText(/tokens\/s$/)).toBeInTheDocument();
		expect(row.getByText(/^First turn about/)).toHaveAttribute(
			"title",
			expect.stringMatching(/^Estimated from the chip/)
		);
		expect(row.getByText("Apache-2.0")).toBeInTheDocument();
	});

	it("offers each row the buttons its state allows", async () => {
		const source = renderPage();
		const act = vi.spyOn(source, "modelAction");

		const loaded = await group("Qwen3.6-35B-A3B");
		expect(loaded.getByRole("button", { name: "Unload model" })).toBeEnabled();
		expect(loaded.queryByRole("button", { name: "Delete" })).toBeNull();

		const downloading = await group("Qwen3.8-27B");
		expect(downloading.getByRole("progressbar")).toBeInTheDocument();
		fireEvent.click(downloading.getByRole("button", { name: "Pause" }));
		await waitFor(() =>
			expect(act).toHaveBeenCalledWith(MAC, {
				action: "pause_download",
				model: DOWNLOADING_MODEL,
				file: expect.any(String),
			})
		);

		const idle = await group("gpt-oss-20b");
		fireEvent.click(idle.getByRole("button", { name: "Delete" }));
		expect(idle.getByText(/^Delete gpt-oss-20b and free [\d.]+ GB\?$/)).toBeInTheDocument();
		fireEvent.click(idle.getAllByRole("button", { name: "Delete" }).at(-1)!);
		await waitFor(() =>
			expect(act).toHaveBeenCalledWith(MAC, {
				action: "delete_model",
				model: "gpt-oss-20b",
				file: expect.any(String),
			})
		);

		const absent = await group("Ornith-1.5-35B-A3B");
		expect(absent.getByRole("button", { name: "Download" })).toBeEnabled();
		expect(absent.getByText(/ GB download, .+ free$/)).toBeInTheDocument();
	});

	it("shows the managed server's model, never its port", async () => {
		renderPage();
		const managed = await group("Run by MaxHQ");
		expect(managed.getByText("Loaded")).toBeInTheDocument();
		expect(managed.getByText(/^llama\.cpp /)).toBeInTheDocument();
		const text = (await screen.findByRole("region", { name: "Servers" })).textContent ?? "";
		expect(text).not.toMatch(/127\.0\.0\.1|localhost|:\d{4,5}\b/);
	});

	it("saves the default model and the settings with the device's other settings", async () => {
		const source = renderPage();
		const save = vi.spyOn(source, "setDeviceSettings");
		await choose(
			await screen.findByRole("combobox", { name: /^Default model,/ }),
			"gpt-oss-20b"
		);
		await waitFor(() =>
			expect(save).toHaveBeenCalledWith(
				expect.objectContaining({
					device: MAC,
					local_models: expect.objectContaining({
						default_model: { server: MAC_SERVER, model: "gpt-oss-20b@mxfp4" },
					}),
				})
			)
		);

		fireEvent.click(screen.getByRole("switch", { name: "Fewer tools for local agents" }));
		await waitFor(() =>
			expect(save).toHaveBeenLastCalledWith(
				expect.objectContaining({
					local_models: expect.objectContaining({ fewer_tools: false }),
				})
			)
		);

		const idle = screen.getByRole("textbox", { name: "Unload models after" });
		fireEvent.change(idle, { target: { value: "1.5" } });
		fireEvent.keyDown(idle, { key: "Enter" });
		expect(
			await screen.findByText("Type a whole number of minutes from 0 to 1,440.")
		).toBeInTheDocument();
	});

	it("lets the Mac keep its models from linked devices, on by default", async () => {
		const source = renderPage();
		const save = vi.spyOn(source, "setDeviceSettings");
		const servers = within(await screen.findByRole("region", { name: "Servers" }));
		const sharing = servers.getByRole("switch", {
			name: "Linked devices can use this Mac's models",
		});
		expect(sharing).toBeChecked();
		expect(servers.getByText(/over the maxd link\. Nothing listens/)).toBeInTheDocument();
		fireEvent.click(sharing);
		await waitFor(() =>
			expect(save).toHaveBeenLastCalledWith(
				expect.objectContaining({
					device: MAC,
					local_models: expect.objectContaining({
						linked_devices: false,
						fewer_tools: true,
					}),
				})
			)
		);
		await waitFor(() => expect(sharing).not.toBeChecked());

		// devbox runs no server of its own, so it has nothing to share.
		await showDevbox();
		await screen.findByRole("region", { name: "Hardware of devbox" });
		expect(
			screen.queryByRole("switch", { name: "Linked devices can use this Mac's models" })
		).toBeNull();
	});

	it("says devbox is too slow for agents and points to a server URL", async () => {
		renderPage();
		await showDevbox();
		const hardware = within(await screen.findByRole("region", { name: "Hardware of devbox" }));
		expect(hardware.getByText("Too slow for agents")).toBeInTheDocument();
		expect(hardware.getByText(/no GPU MaxHQ can use/)).toBeInTheDocument();
		expect(screen.queryByRole("region", { name: "Models" })).toBeNull();
		expect(screen.queryByRole("textbox", { name: "Unload models after" })).toBeNull();

		const ollama = await group("Ollama on gpu-box");
		expect(ollama.getByText(OLLAMA_URL)).toHaveClass("selectable");
		expect(ollama.getByText("Not encrypted on the network").parentElement).toHaveAttribute(
			"title",
			expect.stringMatching(/^Plain http to another machine/)
		);
	});

	const openForm = async () => {
		fireEvent.click(
			within(await screen.findByRole("region", { name: "Servers" })).getByRole("button", {
				name: "Add server URL",
			})
		);
		return group("Add server URL");
	};

	/** The servers devbox has in the source's view. */
	const devboxServers = async (source: DataSource) =>
		(await source.snapshot()).entities.filter(
			(e) => e.entity === "model_server" && e.device === DEVBOX
		).length;

	it("tests a server URL with its token and saves nothing until Add", async () => {
		const source = renderPage();
		const act = vi.spyOn(source, "modelAction");
		await showDevbox();
		const before = await devboxServers(source);
		const form = await openForm();
		const test = form.getByRole("button", { name: "Test" });
		expect(test).toBeDisabled();

		const url = "http://lab-box.example.com:11434";
		fireEvent.change(form.getByRole("textbox", { name: "URL" }), { target: { value: url } });
		expect(form.getByText("Not encrypted on the network")).toBeInTheDocument();
		expect(form.getByText(/You can still add it\.$/)).toBeInTheDocument();
		const token = form.getByLabelText("Token, if the server asks for one") as HTMLInputElement;
		fireEvent.change(token, { target: { value: "tok-123" } });

		fireEvent.click(test);
		const found = within(await form.findByRole("status", { name: "What the test found" }));
		expect(act).toHaveBeenCalledWith(DEVBOX, {
			action: "probe_server",
			url,
			token: "tok-123",
		});
		expect(found.getByText("Ollama")).toBeInTheDocument();
		expect(found.getByText(/^Works with Local agents/)).toBeInTheDocument();
		expect(found.getByText(/^Offers /)).toBeInTheDocument();
		expect(document.body.textContent).not.toContain("tok-123");
		// Nothing saved: no add, and devbox has the servers it had.
		expect(act).toHaveBeenCalledTimes(1);
		expect(await devboxServers(source)).toBe(before);

		// Add saves it, with the token, and clears the field.
		fireEvent.click(form.getByRole("button", { name: "Add" }));
		await waitFor(() =>
			expect(act).toHaveBeenLastCalledWith(DEVBOX, {
				action: "add_server",
				label: "lab-box.example.com",
				url,
				token: "tok-123",
			})
		);
		expect(token.value).toBe("");
		await waitFor(() =>
			expect(screen.queryByRole("group", { name: "Add server URL" })).toBeNull()
		);
		expect(await devboxServers(source)).toBe(before + 1);
	});

	it("says what a Test found wrong, and Cancel sends nothing", async () => {
		const source = renderPage();
		const act = vi.spyOn(source, "modelAction");
		await showDevbox();
		const before = await devboxServers(source);
		const form = await openForm();
		const url = "http://nothing.invalid:8080";
		fireEvent.change(form.getByRole("textbox", { name: "URL" }), { target: { value: url } });
		fireEvent.click(form.getByRole("button", { name: "Test" }));
		const found = within(await form.findByRole("status", { name: "What the test found" }));
		expect(found.getByText(`Nothing answered at ${url}`)).toHaveClass("text-status-error-text");
		// A new URL clears what the last Test found.
		fireEvent.change(form.getByRole("textbox", { name: "URL" }), {
			target: { value: "http://other.invalid:8080" },
		});
		expect(form.queryByRole("status", { name: "What the test found" })).toBeNull();

		fireEvent.click(form.getByRole("button", { name: "Cancel" }));
		await waitFor(() =>
			expect(screen.queryByRole("group", { name: "Add server URL" })).toBeNull()
		);
		expect(act).toHaveBeenCalledTimes(1);
		expect(await devboxServers(source)).toBe(before);
	});

	it("says when the device's maxd is too old for local models", async () => {
		renderPage(mock(oldMaxd));
		await showDevbox();
		expect(
			await screen.findByText("devbox's maxd is too old for these. Update it first.")
		).toBeInTheDocument();
		expect(screen.queryByRole("region", { name: "Servers" })).toBeNull();
	});
});
