// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, ModelFile, ModelLoad } from "../bindings";
import {
	DOWNLOADING_MODEL,
	LLAMA_CPP_VERSION,
	LOADED_CATALOG_MODEL,
	LOADED_MODEL,
	MAC_SERVER,
	OLLAMA_SERVER,
	OLLAMA_URL,
} from "../data/mock-local-models";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import { LOCAL_MODEL_DEFAULTS } from "./models";
import { FIT_TONE } from "./picker";
import { catalogRows, type CatalogRow } from "./select";
import {
	addedServerOf,
	addServerAction,
	AUTO_DEFAULT,
	canAddServer,
	contextOptions,
	defaultLocalDevice,
	defaultModelOptions,
	defaultModelValue,
	deleteQuestion,
	deviceState,
	hardwareText,
	localDeviceOptions,
	managedServerView,
	modelRow,
	parseIdleMinutes,
	pageDevice,
	pickOf,
	pickValue,
	probeSummary,
	rowModelAction,
	serverLabelFor,
	serverUrlProblem,
	typedNotEncrypted,
} from "./settings-page";
import { fitLabel, gpuLimitText, runtimeSupportText } from "./wording";

const snapshot = () => mockSource({ now: () => NOW, tick: false }).snapshot();

const load = async (): Promise<ViewState> => applySnapshot(await snapshot());

/** gpt-oss-20b's ID on the Mac's managed server. */
const GPT_OSS = "gpt-oss-20b@mxfp4";

/** The sample view with its entities changed. */
const edited = async (edit: (entities: Entity[]) => Entity[]): Promise<ViewState> => {
	const taken = await snapshot();
	return applySnapshot({ ...taken, entities: edit(taken.entities) });
};

/** The sample view with one of the Mac's files in another state. */
const withFile = (model: string, patch: Partial<ModelFile>) =>
	edited((entities) =>
		entities.map((e) =>
			e.entity === "local_models" && e.device === MAC
				? { ...e, files: e.files.map((f) => (f.model === model ? { ...f, ...patch } : f)) }
				: e
		)
	);

/** The sample view with a model on the Mac's server in another load state. */
const withLoad = (model: string, load: ModelLoad) =>
	edited((entities) =>
		entities.map((e) =>
			e.entity === "model_server" && e.id === MAC_SERVER
				? { ...e, models: e.models.map((m) => (m.id === model ? { ...m, load } : m)) }
				: e
		)
	);

const rowOf = (view: ViewState, model: string): CatalogRow => {
	const found = catalogRows(view, MAC).find((r) => r.model.id === model);
	if (!found) throw new Error(`No row for ${model}`);
	return found;
};

const shown = (view: ViewState, model: string) => modelRow(view, MAC, rowOf(view, model));

describe("the device the page opens on", () => {
	it("is this Mac first, then the first device with local models", async () => {
		const view = await load();
		expect(defaultLocalDevice(view)).toBe(MAC);
		expect(localDeviceOptions(view).map((o) => [o.value, o.detail])).toEqual([
			[MAC, "This Mac"],
			[DEVBOX, undefined],
		]);
		expect(pageDevice(view, DEVBOX)).toBe(DEVBOX);
		expect(pageDevice(view, "dev-gone")).toBe(MAC);

		const noMac = await edited((entities) =>
			entities.filter((e) => !(e.entity === "device_link" && e.device === MAC))
		);
		expect(defaultLocalDevice(noMac)).toBe(DEVBOX);
	});

	it("says when a device's maxd is too old for local models", async () => {
		const view = await edited((entities) =>
			entities.map((e) =>
				e.entity === "device_link" && e.device === DEVBOX && e.capabilities
					? { ...e, capabilities: { ...e.capabilities, local_models: false } }
					: e
			)
		);
		expect(deviceState(view, DEVBOX)).toEqual({ state: "too_old" });
		expect(deviceState(view, MAC).state).toBe("ready");
	});
});

describe("the hardware", () => {
	it("names the chip, GPU and memory, and the GPU limit MaxHQ leaves alone", async () => {
		const view = await load();
		const mac = view.tables.local_models.get(MAC)!;
		expect(hardwareText(mac.hardware)).toBe("Apple M4 Max, 32-core GPU, 36 GB memory");
		expect(gpuLimitText(mac.hardware)).toMatch(/freeze the Mac/);
		const devbox = view.tables.local_models.get(DEVBOX)!;
		expect(hardwareText(devbox.hardware)).toBe("64 GB memory");
		expect(runtimeSupportText(devbox.runtime.support)).toMatch(/^Too slow for agents/);
		expect(runtimeSupportText("url_only")).toMatch(/Add a server URL instead/);
	});
});

describe("a catalog row", () => {
	it("offers a download with its size and the free space", async () => {
		const view = await load();
		const row = shown(view, "ornith-1.5-35b-a3b");
		expect(row.buttons).toEqual(["download"]);
		expect(row.disk).toEqual({ text: "21.7 GB download, 227.6 GB free", short: false });
		expect(rowModelAction(rowOf(view, "ornith-1.5-35b-a3b"), row, "download")).toEqual({
			action: "download",
			model: "ornith-1.5-35b-a3b",
			file: "Ornith-1.5-35B-A3B-Q4_K_M.gguf",
		});
	});

	it("warns when the disk is too full, and still offers the download", async () => {
		const view = await edited((entities) =>
			entities.map((e) =>
				e.entity === "local_models" && e.device === MAC ? { ...e, disk_free: 5e9 } : e
			)
		);
		const row = shown(view, "ornith-1.5-35b-a3b");
		expect(row.disk).toEqual({ text: "Needs 21.7 GB, but only 5 GB is free", short: true });
		expect(row.buttons).toEqual(["download"]);
	});

	it("shows a download's progress with Pause and Delete", async () => {
		const view = await load();
		const row = shown(view, DOWNLOADING_MODEL);
		expect(row.status).toMatch(/^Downloading, \d+%$/);
		expect(row.percent).toBeGreaterThan(0);
		expect(row.buttons).toEqual(["pause", "delete"]);
		expect(rowModelAction(rowOf(view, DOWNLOADING_MODEL), row, "pause")).toMatchObject({
			action: "pause_download",
			model: DOWNLOADING_MODEL,
		});
		expect(deleteQuestion(rowOf(view, DOWNLOADING_MODEL))).toBe(
			"Stop the download and delete the 7.3 GB downloaded so far?"
		);
	});

	it("waits while the download is checked", async () => {
		const view = await withFile(DOWNLOADING_MODEL, {
			state: { state: "downloading", phase: "verifying" },
		});
		const row = shown(view, DOWNLOADING_MODEL);
		expect(row).toMatchObject({ status: "Checking the download", busy: true, buttons: [] });
	});

	it("resumes a paused download, and retries a failed one", async () => {
		const paused = shown(
			await withFile(DOWNLOADING_MODEL, { state: { state: "paused" } }),
			DOWNLOADING_MODEL
		);
		expect(paused.status).toMatch(/^Paused, \d+%$/);
		expect(paused.buttons).toEqual(["resume", "delete"]);

		const failedView = await withFile(DOWNLOADING_MODEL, {
			state: { state: "failed", reason: "The checksum didn't match" },
		});
		const failed = shown(failedView, DOWNLOADING_MODEL);
		expect(failed).toMatchObject({
			status: "Download failed: The checksum didn't match",
			error: true,
			buttons: ["retry", "delete"],
		});
		expect(rowModelAction(rowOf(failedView, DOWNLOADING_MODEL), failed, "retry")).toMatchObject(
			{
				action: "resume_download",
			}
		);
	});

	it("loads a downloaded model, and unloads a loaded one", async () => {
		const view = await load();
		const idle = shown(view, "gpt-oss-20b");
		expect(idle).toMatchObject({ status: "Not loaded", buttons: ["load", "delete"] });
		expect(rowModelAction(rowOf(view, "gpt-oss-20b"), idle, "load")).toEqual({
			action: "load",
			server: MAC_SERVER,
			model: GPT_OSS,
		});
		const loaded = shown(view, LOADED_CATALOG_MODEL);
		expect(loaded).toMatchObject({ status: "Loaded", buttons: ["unload"] });
		expect(rowModelAction(rowOf(view, LOADED_CATALOG_MODEL), loaded, "unload")).toEqual({
			action: "unload",
			server: MAC_SERVER,
			model: LOADED_MODEL,
		});
		expect(deleteQuestion(rowOf(view, "gpt-oss-20b"))).toMatch(
			/^Delete .+ and free [\d.]+ GB\?$/
		);
	});

	it("waits while a model loads, and offers Load again after a failed load", async () => {
		const loading = shown(await withLoad(GPT_OSS, { state: "loading" }), "gpt-oss-20b");
		expect(loading).toMatchObject({
			status: "Loading model",
			busy: true,
			buttons: [],
			percent: null,
		});
		const partway = shown(
			await withLoad(GPT_OSS, { state: "loading", percent: 42 }),
			"gpt-oss-20b"
		);
		expect(partway).toMatchObject({
			status: "Loading model, 42%",
			percent: 42,
			loading: true,
		});
		const warming = shown(
			await withLoad(GPT_OSS, { state: "loading", percent: 99 }),
			"gpt-oss-20b"
		);
		expect(warming).toMatchObject({ status: "Warming up", percent: 99, loading: true });
		const failed = shown(
			await withLoad(GPT_OSS, { state: "failed", reason: "Out of memory" }),
			"gpt-oss-20b"
		);
		expect(failed).toMatchObject({
			status: "Couldn't load: Out of memory",
			error: true,
			buttons: ["load", "delete"],
		});
	});

	it("offers only Delete for a file the server doesn't list yet", async () => {
		const view = await edited((entities) =>
			entities.map((e) =>
				e.entity === "model_server" && e.id === MAC_SERVER
					? { ...e, models: e.models.filter((m) => m.id !== GPT_OSS) }
					: e
			)
		);
		const row = shown(view, "gpt-oss-20b");
		expect(row).toMatchObject({ status: "Downloaded", buttons: ["delete"], served: null });
		expect(rowModelAction(rowOf(view, "gpt-oss-20b"), row, "load")).toBeNull();
	});

	it("colors the fit pill by how well the model fits", async () => {
		const view = await load();
		const fits = catalogRows(view, MAC).map((r) => r.estimate?.fit);
		expect(fits).toContain("fits");
		expect(fits).toContain("too_large");
		expect([FIT_TONE.fits, fitLabel("fits")]).toEqual(["good", "Fits"]);
		expect([FIT_TONE.tight, fitLabel("tight")]).toEqual(["warn", "Tight"]);
		expect([FIT_TONE.too_large, fitLabel("too_large")]).toEqual(["bad", "Too large"]);
	});
});

describe("the servers", () => {
	it("summarise the managed server without its port or key", async () => {
		const view = await load();
		const server = view.tables.model_server.get(MAC_SERVER)!;
		const summary = managedServerView(view, server);
		expect(summary).toEqual({
			state: `llama.cpp ${LLAMA_CPP_VERSION}`,
			error: false,
			model: { name: "Qwen3.6-35B-A3B", load: "Loaded", tone: "good" },
			users: "Local agents on devbox use it too",
			problem: null,
		});
		expect(JSON.stringify(summary)).not.toMatch(/:\d{2,5}\b|127\.0\.0\.1|localhost/);
	});

	it("say when the managed server failed to set up", async () => {
		const view = await edited((entities) =>
			entities.map((e) =>
				e.entity === "local_models" && e.device === MAC
					? {
							...e,
							runtime: {
								...e.runtime,
								state: { type: "failed", reason: "No space left" },
							},
						}
					: e
			)
		);
		const summary = managedServerView(view, view.tables.model_server.get(MAC_SERVER)!);
		expect(summary).toMatchObject({ state: "Couldn't set it up: No space left", error: true });
	});

	it("sum up a server's last test", async () => {
		const view = await load();
		const ollama = view.tables.model_server.get(OLLAMA_SERVER)!;
		expect(probeSummary(ollama)).toEqual({ text: "Works with Local agents", ok: true });
		expect(probeSummary(view.tables.model_server.get(MAC_SERVER)!)).toEqual({
			text: "Works with Local agents, 65,536 tokens of context",
			ok: true,
		});
		expect(probeSummary({ probe: null })).toEqual({ text: "Not tested yet", ok: null });
		expect(probeSummary({ probe: { ...ollama.probe!, apis: { chat: true } } }).ok).toBe(false);
		expect(probeSummary({ probe: { ...ollama.probe!, error: "Nothing answered" } })).toEqual({
			text: "Nothing answered",
			ok: false,
		});
	});
});

describe("the form that adds a server", () => {
	it("checks the URL, and notes plain http to another machine", () => {
		expect(serverUrlProblem("")).toBeNull();
		expect(serverUrlProblem("gpu-box:11434")).toMatch(/http:\/\/ or https:\/\//);
		expect(serverUrlProblem("ftp://gpu-box")).not.toBeNull();
		expect(canAddServer(OLLAMA_URL)).toBe(true);
		expect(canAddServer("  ")).toBe(false);
		expect(typedNotEncrypted(OLLAMA_URL)).toBe(true);
		expect(typedNotEncrypted("http://localhost:8080")).toBe(false);
		expect(typedNotEncrypted("https://gpu-box.example.com")).toBe(false);
	});

	it("sends the label, else the host, and the token only when typed", () => {
		expect(serverLabelFor("", OLLAMA_URL)).toBe("gpu-box.example.com");
		expect(addServerAction(" Lab ", ` ${OLLAMA_URL} `, "")).toEqual({
			action: "add_server",
			label: "Lab",
			url: OLLAMA_URL,
			token: null,
		});
		expect(addServerAction("", OLLAMA_URL, "tok-123")).toMatchObject({ token: "tok-123" });
		expect(
			addedServerOf({ status: "ok", data: { status: "model_server_added", server: "srv-1" } })
		).toBe("srv-1");
		expect(addedServerOf({ status: "ok", data: { status: "done" } })).toBeNull();
	});
});

describe("the default model", () => {
	it("offers Automatic, then every model under its server", async () => {
		const view = await load();
		const options = defaultModelOptions(view, DEVBOX, LOCAL_MODEL_DEFAULTS);
		expect(options[0]).toMatchObject({
			value: AUTO_DEFAULT,
			label: "Automatic",
			detail: "Qwen3.6-35B-A3B",
		});
		expect(options.map((o) => o.group)).toContain("Ollama on gpu-box (not encrypted)");
		expect(options.map((o) => o.group)).toContain("This Mac");
		const value = pickValue(MAC_SERVER, LOADED_MODEL);
		expect(options.some((o) => o.value === value)).toBe(true);
		expect(pickOf(view, DEVBOX, value)?.model.id).toBe(LOADED_MODEL);
		expect(pickOf(view, DEVBOX, AUTO_DEFAULT)).toBeNull();
	});

	it("keeps naming a saved default that's gone", async () => {
		const view = await load();
		const settings = { default_model: { server: MAC_SERVER, model: "gone" } };
		expect(defaultModelValue(settings)).toBe(pickValue(MAC_SERVER, "gone"));
		expect(defaultModelOptions(view, MAC, settings).at(-1)).toMatchObject({
			label: "gone",
			warn: true,
		});
	});
});

describe("the settings", () => {
	it("take whole minutes for the idle unload", () => {
		expect(parseIdleMinutes(" 30 ")).toEqual({ ok: true, minutes: 30 });
		expect(parseIdleMinutes("0")).toEqual({ ok: true, minutes: 0 });
		expect(parseIdleMinutes("1441").ok).toBe(false);
		expect(parseIdleMinutes("1.5").ok).toBe(false);
		expect(parseIdleMinutes("").ok).toBe(false);
	});

	it("offer the context sizes with the current one among them", () => {
		expect(contextOptions(65_536).map((o) => o.value)).toEqual([
			"16384",
			"32768",
			"65536",
			"131072",
			"262144",
		]);
		expect(contextOptions(65_536).find((o) => o.value === "65536")?.detail).toBe("The default");
		expect(contextOptions(50_000).map((o) => o.value)).toContain("50000");
	});
});
