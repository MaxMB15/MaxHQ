// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";

import { NOW } from "../test/build";
import { applySnapshot } from "../view/replica";
import {
	DOWNLOADING_MODEL,
	LOADED_CATALOG_MODEL,
	LOADED_MODEL,
	MAC_SERVER,
	mockCatalog,
	OLLAMA_SERVER,
	OPENROUTER_SERVER,
} from "./mock-local-models";
import { mockSource } from "./mock-source";
import { DEVBOX, MAC } from "./mock-view";

const source = () => mockSource({ now: () => NOW, tick: false, stepMs: 10 });

const view = async (mock: ReturnType<typeof source>) => applySnapshot(await mock.snapshot());

afterEach(() => vi.useRealTimers());

describe("mock local models", () => {
	it("has six catalog models, the default first, each pinned with checksums", () => {
		const catalog = mockCatalog();
		expect(catalog).toHaveLength(6);
		expect(catalog[0]?.id).toBe(LOADED_CATALOG_MODEL);
		for (const model of catalog) {
			expect(model.commit).toMatch(/^[0-9a-f]{40}$/);
			for (const file of model.files) expect(file.sha256).toMatch(/^[0-9a-f]{64}$/);
		}
	});

	it("adds a server URL, probes it and gives it a login", async () => {
		vi.useFakeTimers();
		const mock = source();
		const result = await mock.modelAction(DEVBOX, {
			action: "add_server",
			label: "",
			url: "http://devbox:1234/",
		});
		expect(result).toMatchObject({ status: "ok", data: { status: "model_server_added" } });
		const id =
			result.status === "ok" && result.data.status === "model_server_added"
				? result.data.server
				: "";
		const before = (await view(mock)).tables.model_server.get(id)!;
		expect(before).toMatchObject({
			label: "devbox:1234",
			base_url: "http://devbox:1234",
			probe: null,
		});
		await vi.runAllTimersAsync();
		const after = await view(mock);
		expect(after.tables.model_server.get(id)).toMatchObject({
			kind: { type: "external", flavor: "lm_studio" },
			probe: { apis: { messages: false } },
		});
		expect([...after.tables.account.values()].some((a) => a.model_server === id)).toBe(true);
	});

	it("keeps an OpenRouter admin key it accepts, reads the balance and removes it", async () => {
		const mock = source();
		const set = (key: string | null, server = OPENROUTER_SERVER) =>
			mock.modelAction(MAC, { action: "set_server_admin_key", server, key });
		const credits = async () => (await view(mock)).tables.model_server.get(OPENROUTER_SERVER)!;
		expect(await set("bad-key")).toMatchObject({ data: { status: "refused" } });
		const refused = await credits();
		expect(refused.admin_key).toBe(false);
		expect(refused.credits?.balance_cents).toBeUndefined();
		expect(await set("sk-or-v1-example", MAC_SERVER)).toMatchObject({
			data: { status: "refused", reason: "Only an OpenRouter server takes an admin key." },
		});
		expect(await set("sk-or-v1-example")).toMatchObject({ data: { status: "done" } });
		expect(await credits()).toMatchObject({
			admin_key: true,
			credits: { balance_cents: 1_484, bought_cents: 2_000, used_cents: 258 },
		});
		expect(await set(null)).toMatchObject({ data: { status: "done" } });
		expect(await credits()).toMatchObject({
			admin_key: false,
			credits: { balance_cents: null, bought_cents: null, used_cents: 258 },
		});
	});

	it("probes a URL without adding it", async () => {
		const mock = source();
		const before = (await view(mock)).tables.model_server.size;
		expect(
			await mock.modelAction(DEVBOX, { action: "probe_server", url: "http://devbox:11434" })
		).toMatchObject({
			status: "ok",
			data: {
				status: "server_probed",
				flavor: "ollama",
				probe: { error: null },
				models: [{ id: "qwen3.6:35b" }],
			},
		});
		expect(
			await mock.modelAction(DEVBOX, {
				action: "probe_server",
				url: "http://gone.invalid:8080",
			})
		).toMatchObject({ data: { status: "server_probed", probe: { apis: {} }, models: [] } });
		expect(
			await mock.modelAction(DEVBOX, { action: "probe_server", url: "devbox:11434" })
		).toMatchObject({ data: { status: "refused" } });
		expect((await view(mock)).tables.model_server.size).toBe(before);
	});

	it("refuses what isn't a server URL, and removing the managed server", async () => {
		const mock = source();
		expect(
			await mock.modelAction(DEVBOX, {
				action: "add_server",
				label: "x",
				url: "devbox:11434",
			})
		).toMatchObject({ data: { status: "refused" } });
		expect(
			await mock.modelAction(DEVBOX, {
				action: "add_server",
				label: "x",
				url: "ftp://devbox",
			})
		).toMatchObject({ data: { status: "refused" } });
		expect(
			await mock.modelAction(MAC, { action: "remove_server", server: MAC_SERVER })
		).toMatchObject({ data: { status: "refused" } });
		await mock.modelAction(DEVBOX, { action: "remove_server", server: OLLAMA_SERVER });
		const after = await view(mock);
		expect(after.tables.model_server.has(OLLAMA_SERVER)).toBe(false);
		expect(
			[...after.tables.account.values()].some((a) => a.model_server === OLLAMA_SERVER)
		).toBe(false);
	});

	it("finishes a download, checks it and adds it to the Mac's server", async () => {
		vi.useFakeTimers();
		const mock = source();
		const file = "Qwen3.8-27B-UD-Q4_K_XL.gguf";
		await mock.modelAction(MAC, { action: "pause_download", model: DOWNLOADING_MODEL, file });
		const paused = (await view(mock)).tables.local_models.get(MAC)!.files;
		expect(paused.find((f) => f.model === DOWNLOADING_MODEL)?.state.state).toBe("paused");
		await mock.modelAction(MAC, { action: "resume_download", model: DOWNLOADING_MODEL, file });
		await vi.runAllTimersAsync();
		const done = await view(mock);
		expect(
			done.tables.local_models.get(MAC)!.files.find((f) => f.model === DOWNLOADING_MODEL)
		).toMatchObject({ state: { state: "ready" } });
		expect(
			done.tables.model_server
				.get(MAC_SERVER)!
				.models.find((m) => m.id === `${DOWNLOADING_MODEL}@ud-q4_k_xl`)
		).toMatchObject({
			catalog: { model: DOWNLOADING_MODEL, file },
			load: { state: "unloaded" },
		});
	});

	it("refuses a download on a device without a GPU", async () => {
		const mock = source();
		const model = mockCatalog()[0]!;
		expect(
			await mock.modelAction(DEVBOX, {
				action: "download",
				model: model.id,
				file: model.files[0]!.name,
			})
		).toMatchObject({ data: { status: "refused" } });
	});

	it("loads one model at a time and won't delete a loaded one", async () => {
		vi.useFakeTimers();
		const mock = source();
		expect(
			await mock.modelAction(MAC, {
				action: "delete_model",
				model: LOADED_CATALOG_MODEL,
				file: "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf",
			})
		).toMatchObject({ data: { status: "refused" } });
		const gptOss = "gpt-oss-20b@mxfp4";
		await mock.modelAction(MAC, { action: "load", server: MAC_SERVER, model: gptOss });
		const loading = (await view(mock)).tables.model_server.get(MAC_SERVER)!.models;
		expect(loading.find((m) => m.id === gptOss)?.load?.state).toBe("loading");
		expect(loading.find((m) => m.id === LOADED_MODEL)?.load?.state).toBe("unloaded");
		await vi.advanceTimersByTimeAsync(6);
		const halfway = (await view(mock)).tables.model_server.get(MAC_SERVER)!.models;
		expect(halfway.find((m) => m.id === gptOss)?.load).toEqual({
			state: "loading",
			percent: 50,
		});
		await vi.runAllTimersAsync();
		const loaded = (await view(mock)).tables.model_server.get(MAC_SERVER)!.models;
		expect(loaded.find((m) => m.id === gptOss)?.load).toMatchObject({
			state: "loaded",
			context: 65_536,
		});
		expect(
			await mock.modelAction(DEVBOX, {
				action: "load",
				server: OLLAMA_SERVER,
				model: "gpt-oss:20b",
			})
		).toMatchObject({ data: { status: "refused" } });
	});
});
