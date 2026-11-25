// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { openSettingsWindow } from "@/test/settings";
import { CLOUD_SERVERS_SINCE, type Entity } from "@maxhq/shared/bindings";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import {
	DEVBOX_OPENROUTER_SERVER,
	DOWNLOADING_MODEL,
	MAC_SERVER,
	OLLAMA_URL,
	OPENROUTER_SERVER,
} from "@maxhq/shared/data/mock-local-models";
import { OPENROUTER_URL } from "@maxhq/shared/local-models/openrouter";
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
		const { dialog, nav } = await openSettingsWindow();
		const field = within(nav).getByRole("searchbox", { name: "Search settings" });
		const views = () => within(within(nav).getByRole("group", { name: "Their own views" }));
		for (const words of ["ollama", "llama.cpp", "GPU", "KV cache"]) {
			fireEvent.change(field, { target: { value: words } });
			expect(views().getByRole("button", { name: /^Local models/ }), words).toBeVisible();
		}
		fireEvent.click(views().getByRole("button", { name: /^Local models/ }));
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
		// A server you run is no cloud service.
		expect(ollama.queryByText("Cloud service")).toBeNull();
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

	it("turns Cloud service on when Test finds one, until you set it yourself", async () => {
		const source = renderPage();
		const act = vi.spyOn(source, "modelAction");
		// The Mac's OpenRouter says it is a cloud service, for cloud agents.
		const row = await group("OpenRouter");
		expect(row.getByText("Cloud service")).toBeInTheDocument();
		expect(row.getByText(/^Works with cloud agents/)).toBeInTheDocument();
		expect(row.queryByText(/Local agents/)).toBeNull();

		const form = await openForm();
		const cloud = form.getByRole("switch", { name: "Cloud service" });
		expect(form.getByText(/^A hosted API, not hardware you run\./)).toBeInTheDocument();
		expect(cloud).not.toBeChecked();
		const url = "https://openrouter.ai/api";
		fireEvent.change(form.getByRole("textbox", { name: "URL" }), { target: { value: url } });
		const test = form.getByRole("button", { name: "Test" });
		fireEvent.click(test);
		const found = within(await form.findByRole("status", { name: "What the test found" }));
		expect(cloud).toBeChecked();
		expect(found.getByText(/^Works with cloud agents/)).toBeInTheDocument();

		// Once you set it, a Test leaves it as you set it.
		fireEvent.click(cloud);
		expect(cloud).not.toBeChecked();
		fireEvent.click(test);
		await waitFor(() => expect(act).toHaveBeenCalledTimes(2));
		await form.findByRole("status", { name: "What the test found" });
		expect(cloud).not.toBeChecked();

		fireEvent.click(cloud);
		fireEvent.click(form.getByRole("button", { name: "Add" }));
		await waitFor(() =>
			expect(act).toHaveBeenLastCalledWith(
				MAC,
				expect.objectContaining({ action: "add_server", url, cloud: true })
			)
		);
	});

	it("words the Test result for cloud agents while Cloud service is on", async () => {
		renderPage();
		const form = await openForm();
		fireEvent.change(form.getByRole("textbox", { name: "URL" }), {
			target: { value: "http://lab-box.example.com:11434" },
		});
		fireEvent.click(form.getByRole("button", { name: "Test" }));
		const found = within(await form.findByRole("status", { name: "What the test found" }));
		expect(found.getByText(/^Works with Local agents/)).toBeInTheDocument();
		fireEvent.click(form.getByRole("switch", { name: "Cloud service" }));
		expect(found.getByText(/^Works with cloud agents/)).toBeInTheDocument();
		expect(found.queryByText(/Local agents/)).toBeNull();
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

describe("OpenRouter in the Servers card", () => {
	const openForm = async () => {
		fireEvent.click(
			within(await screen.findByRole("region", { name: "Servers" })).getByRole("button", {
				name: "Add server URL",
			})
		);
		return group("Add server URL");
	};

	/** The form, with OpenRouter chosen. */
	const openOpenRouter = async () => {
		const form = await openForm();
		const kinds = within(form.getByRole("group", { name: "Kind of server" }));
		expect(kinds.getByRole("button", { name: "Your own server" })).toHaveAttribute(
			"aria-pressed",
			"true"
		);
		fireEvent.click(kinds.getByRole("button", { name: "OpenRouter" }));
		expect(kinds.getByRole("button", { name: "OpenRouter" })).toHaveAttribute(
			"aria-pressed",
			"true"
		);
		return form;
	};

	// The mock source with the Mac's maxd from before cloud servers.
	const beforeSignIn = (source: DataSource): DataSource => ({
		...source,
		snapshot: async () => {
			const snapshot = await source.snapshot();
			const entities = snapshot.entities.map((e): Entity =>
				e.entity === "device_link" && e.device === MAC
					? { ...e, protocol: CLOUD_SERVERS_SINCE - 1 }
					: e
			);
			return { ...snapshot, entities };
		},
	});

	it("sets OpenRouter's URL and offers Sign in first, then a pasted key", async () => {
		renderPage();
		const form = await openOpenRouter();
		expect(form.queryByRole("textbox", { name: "URL" })).toBeNull();
		expect(form.queryByRole("textbox", { name: "Name" })).toBeNull();
		expect(form.getByText(OPENROUTER_URL)).toHaveClass("selectable");
		// The Mac has an OpenRouter server, which a sign-in gives a new key.
		expect(
			form.getByText("Signing in gives OpenRouter on this device a new key.")
		).toBeInTheDocument();
		// maxd knows OpenRouter is a cloud service, so there's no switch.
		expect(form.queryByRole("switch", { name: "Cloud service" })).toBeNull();
		expect(form.getByRole("textbox", { name: "Spending limit" })).toHaveValue("");
		expect(form.getByText("OpenRouter stops this key at the limit")).toBeInTheDocument();
		expect(form.getByRole("button", { name: "Sign in with OpenRouter" })).toBeEnabled();
		expect(form.getByText("Or paste an API key")).toBeInTheDocument();
		expect(form.getByRole("link", { name: "Get a key" })).toHaveAttribute(
			"href",
			"https://openrouter.ai/settings/keys"
		);
		expect(form.getByLabelText("API key")).toHaveAttribute("type", "password");
		// Test and Add wait for a key.
		expect(form.getByRole("button", { name: "Test" })).toBeDisabled();
		expect(form.getByRole("button", { name: "Add" })).toBeDisabled();

		// Back to your own server, with its fields empty.
		fireEvent.click(form.getByRole("button", { name: "Your own server" }));
		expect(form.getByRole("textbox", { name: "URL" })).toHaveValue("");
		expect(form.queryByRole("button", { name: "Sign in with OpenRouter" })).toBeNull();
		expect(form.getByRole("switch", { name: "Cloud service" })).not.toBeChecked();
	});

	it("says a limit it can't send, and keeps Sign in off", async () => {
		renderPage();
		const form = await openOpenRouter();
		const limit = form.getByRole("textbox", { name: "Spending limit" });
		fireEvent.change(limit, { target: { value: "ten" } });
		expect(form.getByText("Type an amount in dollars, such as 25 or 12.50.")).toHaveClass(
			"text-status-error-text"
		);
		expect(limit).toHaveAttribute("aria-invalid", "true");
		expect(form.getByRole("button", { name: "Sign in with OpenRouter" })).toBeDisabled();
		fireEvent.change(limit, { target: { value: "12.50" } });
		expect(form.getByRole("button", { name: "Sign in with OpenRouter" })).toBeEnabled();
	});

	it("waits for the browser, says why a round failed, and closes once one works", async () => {
		const source = renderPage();
		const signIn = vi.spyOn(source, "signInOpenRouter");
		const form = await openOpenRouter();
		fireEvent.change(form.getByRole("textbox", { name: "Spending limit" }), {
			target: { value: "$25" },
		});
		fireEvent.click(form.getByRole("button", { name: "Sign in with OpenRouter" }));
		expect(signIn).toHaveBeenCalledWith(MAC, 2500);
		expect(await form.findByText("Finish signing in in your browser")).toBeInTheDocument();
		expect(form.getByRole("textbox", { name: "Spending limit" })).toBeDisabled();
		expect(form.queryByRole("button", { name: "Sign in with OpenRouter" })).toBeNull();

		// The sample's first round fails the way an expired code does.
		expect(await form.findByText(/start the sign-in again\.$/)).toHaveClass(
			"text-status-error-text"
		);
		fireEvent.click(form.getByRole("button", { name: "Try again" }));
		await waitFor(() =>
			expect(screen.queryByRole("group", { name: "Add server URL" })).toBeNull()
		);
		expect(signIn).toHaveBeenCalledTimes(2);
		// The round gave the Mac's OpenRouter server its key, and added none.
		expect((await group("OpenRouter")).getByText(OPENROUTER_URL)).toBeInTheDocument();
		expect(screen.getAllByRole("group", { name: "OpenRouter" })).toHaveLength(1);
	});

	it("cancels a round that waits, and one whose form closes", async () => {
		// A round that waits until it's cancelled, however slow the run is.
		let stop: (() => void) | null = null;
		const source = renderPage(
			mock((s) => ({
				...s,
				signInOpenRouter: () =>
					new Promise<ActionResult>((resolve) => {
						stop = () =>
							resolve({
								status: "error",
								error: { kind: "failed", message: "You cancelled the sign-in" },
							});
					}),
				cancelSignIn: async () => {
					stop?.();
					return { status: "ok", data: null };
				},
			}))
		);
		const cancel = vi.spyOn(source, "cancelSignIn");
		const form = await openOpenRouter();
		fireEvent.click(form.getByRole("button", { name: "Sign in with OpenRouter" }));
		fireEvent.click(await form.findByRole("button", { name: "Cancel the sign-in" }));
		expect(cancel).toHaveBeenCalledTimes(1);
		// A cancelled round says nothing and starts over.
		expect(
			await form.findByRole("button", { name: "Sign in with OpenRouter" })
		).toBeInTheDocument();
		expect(form.queryByText("Finish signing in in your browser")).toBeNull();
		expect(form.queryByRole("status")).toBeNull();

		fireEvent.click(form.getByRole("button", { name: "Sign in with OpenRouter" }));
		await form.findByText("Finish signing in in your browser");
		fireEvent.click(form.getByRole("button", { name: "Cancel" }));
		await waitFor(() =>
			expect(screen.queryByRole("group", { name: "Add server URL" })).toBeNull()
		);
		expect(cancel).toHaveBeenCalledTimes(2);
	});

	it("tests and adds a pasted key at OpenRouter's URL", async () => {
		// devbox without its OpenRouter server.
		const source = mock();
		await source.modelAction(DEVBOX, {
			action: "remove_server",
			server: DEVBOX_OPENROUTER_SERVER,
		});
		renderPage(source);
		const act = vi.spyOn(source, "modelAction");
		await showDevbox();
		const form = await openOpenRouter();
		expect(form.queryByText(/a new key\.$/)).toBeNull();
		const key = form.getByLabelText("API key") as HTMLInputElement;
		fireEvent.change(key, { target: { value: "sk-or-v1-sample" } });
		fireEvent.input(key);
		fireEvent.click(form.getByRole("button", { name: "Test" }));
		const found = within(await form.findByRole("status", { name: "What the test found" }));
		expect(act).toHaveBeenCalledWith(DEVBOX, {
			action: "probe_server",
			url: OPENROUTER_URL,
			token: "sk-or-v1-sample",
		});
		expect(found.getByText(/^Offers /)).toBeInTheDocument();
		expect(found.getByText(/^Works with cloud agents/)).toBeInTheDocument();
		expect(form.getByRole("button", { name: "Add" })).not.toHaveClass("bg-primary");

		fireEvent.click(form.getByRole("button", { name: "Add" }));
		await waitFor(() =>
			expect(act).toHaveBeenLastCalledWith(DEVBOX, {
				action: "add_server",
				label: "OpenRouter",
				url: OPENROUTER_URL,
				token: "sk-or-v1-sample",
			})
		);
		expect(key.value).toBe("");
		expect(document.body.textContent).not.toContain("sk-or-v1-sample");
		await waitFor(() =>
			expect(screen.queryByRole("group", { name: "Add server URL" })).toBeNull()
		);
	});

	it("offers an older maxd a pasted key only", async () => {
		renderPage(mock(beforeSignIn));
		const form = await openOpenRouter();
		expect(
			form.getByText("Update this device's maxd to sign in with OpenRouter.")
		).toBeInTheDocument();
		// It can't sign in, so it doesn't say a sign-in replaces the key.
		expect(form.queryByText(/a new key\.$/)).toBeNull();
		expect(form.queryByRole("button", { name: "Sign in with OpenRouter" })).toBeNull();
		expect(form.queryByRole("textbox", { name: "Spending limit" })).toBeNull();
		expect(form.getByText("Paste an API key")).toBeInTheDocument();
		expect(form.getByRole("button", { name: "Add" })).toHaveClass("bg-primary");
		// Nor does its OpenRouter server's row offer Sign in again.
		const row = await group("OpenRouter");
		expect(row.queryByRole("button", { name: "Sign in again" })).toBeNull();
	});

	it("signs the OpenRouter server's row in again, keeping the server", async () => {
		const source = renderPage();
		const signIn = vi.spyOn(source, "signInOpenRouter");
		const row = await group("OpenRouter");
		expect(row.getByText(OPENROUTER_URL)).toBeInTheDocument();
		// Sign in again sits with the row's other buttons, its credits kept.
		expect(row.getByRole("button", { name: "Test" })).toBeInTheDocument();
		expect(row.getByRole("button", { name: "Remove" })).toBeInTheDocument();
		expect(row.getByText(/\$7\.42 left of \$10\.00/)).toBeInTheDocument();
		fireEvent.click(row.getByRole("button", { name: "Sign in again" }));
		expect(signIn).toHaveBeenCalledWith(MAC, null);
		expect(await row.findByText("Finish signing in in your browser")).toBeInTheDocument();
		expect(await row.findByText(/start the sign-in again\.$/)).toBeInTheDocument();
		fireEvent.click(row.getByRole("button", { name: "Sign in again" }));
		expect(
			await row.findByText(
				"Signed in with a new key. You can revoke the old one on OpenRouter's keys page."
			)
		).toBeInTheDocument();
		const servers = (await source.snapshot()).entities.filter(
			(e) => e.entity === "model_server" && e.device === MAC
		);
		expect(servers.map((e) => e.entity === "model_server" && e.id)).toContain(
			OPENROUTER_SERVER
		);
		expect(row.getByText(/\$7\.42 left of \$10\.00/)).toBeInTheDocument();
		// The form says a sign-in gives that server a new key.
		const form = await openOpenRouter();
		expect(
			form.getByText("Signing in gives OpenRouter on this device a new key.")
		).toBeInTheDocument();
		// Only OpenRouter's row offers Sign in again.
		expect(
			(await group("Run by MaxHQ")).queryByRole("button", { name: "Sign in again" })
		).toBeNull();
	});
});
