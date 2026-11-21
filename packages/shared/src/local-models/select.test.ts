// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity } from "../bindings";
import {
	DEVBOX_LOCAL,
	DEVBOX_LOCAL_AGENT,
	DOWNLOADING_MODEL,
	LOADED_CATALOG_MODEL,
	LOADED_MODEL,
	MAC_LOCAL,
	MAC_LOCAL_AGENT,
	MAC_SERVER,
	OLLAMA_LOCAL,
	OLLAMA_SERVER,
} from "../data/mock-local-models";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { accountOptions } from "../new-agent/spec";
import { deviceSettingsOf } from "../settings/device-settings";
import { agent, NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";
import { modelsOf } from "../view/select";

import {
	agentCatalogModel,
	agentKindLabel,
	catalogRows,
	DEFAULT_CATALOG_MODEL,
	defaultLocalPick,
	downloadsOn,
	localAccountOption,
	localAccountsOn,
	localModelKey,
	localModelName,
	localModelOptions,
	localSettingsOn,
	LOCAL_MODEL_DEFAULTS,
	LOCAL_SETUP_VALUE,
	modelChoice,
	parseLocalModelKey,
	resolveLocal,
	runsLocalModels,
	serverNote,
	serversOf,
	serverUsers,
	shownLocalModel,
	withDefaultLocalModel,
} from "./select";
import { sameLocalSettings, sharingOff } from "./models";

const load = async (): Promise<ViewState> =>
	applySnapshot(await mockSource({ now: () => NOW, tick: false }).snapshot());

/** The sample view with its entities changed. */
const edited = async (edit: (entities: Entity[]) => Entity[]): Promise<ViewState> => {
	const snapshot = await mockSource({ now: () => NOW, tick: false }).snapshot();
	return applySnapshot({ ...snapshot, entities: edit(snapshot.entities) });
};

describe("servers and models per device", () => {
	it("lists each device's own servers, the managed one first", async () => {
		const view = await load();
		expect(serversOf(view, MAC).map((s) => s.id)).toEqual([MAC_SERVER]);
		expect(serversOf(view, DEVBOX).map((s) => s.id)).toEqual([OLLAMA_SERVER]);
		expect(serverUsers(view, view.tables.model_server.get(MAC_SERVER)!)).toEqual([DEVBOX]);
		expect(runsLocalModels(view, MAC)).toBe(true);
	});

	it("keeps other devices' agents off a Mac's server only while its switch is off", async () => {
		const off = (linked: boolean) =>
			edited((entities) =>
				entities.map((e) =>
					e.entity === "device_settings" && e.device === MAC
						? { ...e, local_models: { ...e.local_models, linked_devices: linked } }
						: e
				)
			);
		const mac = (view: ViewState) => view.tables.model_server.get(MAC_SERVER)!;
		const ollama = (view: ViewState) => view.tables.model_server.get(OLLAMA_SERVER)!;
		const shared = await off(true);
		expect(sharingOff(shared, DEVBOX, mac(shared))).toBe(false);
		const kept = await off(false);
		expect(sharingOff(kept, DEVBOX, mac(kept))).toBe(true);
		// Never for the Mac's own agents, nor for a server at a URL.
		expect(sharingOff(kept, MAC, mac(kept))).toBe(false);
		expect(sharingOff(kept, MAC, ollama(kept))).toBe(false);
		const rows = localModelOptions(kept, DEVBOX).filter((o) =>
			o.value.startsWith(localModelKey(DEVBOX_LOCAL, ""))
		);
		expect(rows.map((o) => [o.disabled, o.warn, o.detail])).toEqual([
			[true, true, "Turned off on Mac"],
			[true, true, "Turned off on Mac"],
			[true, true, "Turned off on Mac"],
		]);
	});

	it("lists the local accounts own servers first, then the ones over the link", async () => {
		const view = await load();
		expect(localAccountsOn(view, MAC).map((a) => a.id)).toEqual([MAC_LOCAL]);
		expect(localAccountsOn(view, DEVBOX).map((a) => a.id)).toEqual([
			OLLAMA_LOCAL,
			DEVBOX_LOCAL,
		]);
	});

	it("shows the Mac's catalog with fit, the default first, and its download", async () => {
		const view = await load();
		const rows = catalogRows(view, MAC);
		expect(rows).toHaveLength(6);
		expect(rows[0]).toMatchObject({ model: { id: DEFAULT_CATALOG_MODEL }, isDefault: true });
		expect(rows[0]!.estimate?.fit).toBe("fits");
		expect(rows.find((r) => r.model.id === "qwen3-coder-next-80b-a3b")?.estimate?.fit).toBe(
			"too_large"
		);
		expect(rows.find((r) => r.model.id === DOWNLOADING_MODEL)?.onDisk?.state.state).toBe(
			"downloading"
		);
		expect(downloadsOn(view, MAC).map((f) => f.model)).toEqual([DOWNLOADING_MODEL]);
		expect(catalogRows(view, DEVBOX)).toEqual([]);
	});

	it("fills in the settings a device hasn't saved", async () => {
		const view = await load();
		expect(localSettingsOn(view, DEVBOX)).toEqual(LOCAL_MODEL_DEFAULTS);
		expect(LOCAL_MODEL_DEFAULTS.linked_devices).toBe(true);
		expect(sameLocalSettings({}, { linked_devices: true })).toBe(true);
		expect(sameLocalSettings({}, { linked_devices: false })).toBe(false);
		expect(localSettingsOn(view, MAC).default_model).toEqual({
			server: MAC_SERVER,
			model: LOADED_MODEL,
		});
	});
});

describe("the default pick", () => {
	it("takes the device's setting", async () => {
		const view = await load();
		const pick = defaultLocalPick(view, MAC)!;
		expect([pick.account.id, pick.server.id, pick.model.id]).toEqual([
			MAC_LOCAL,
			MAC_SERVER,
			LOADED_MODEL,
		]);
	});

	it("falls back to the catalog's default, here on the Mac over the link", async () => {
		const view = await load();
		const pick = defaultLocalPick(view, DEVBOX)!;
		expect([pick.account.id, pick.server.id, pick.model.id]).toEqual([
			DEVBOX_LOCAL,
			MAC_SERVER,
			LOADED_MODEL,
		]);
	});

	it("skips a saved default that's gone, and a server that can't run agents", async () => {
		const view = await edited((entities) =>
			entities.map((e) =>
				e.entity === "device_settings" && e.device === MAC
					? {
							...e,
							local_models: { default_model: { server: MAC_SERVER, model: "gone" } },
						}
					: e.entity === "model_server" && e.id === OLLAMA_SERVER && e.probe
						? { ...e, probe: { ...e.probe, apis: { chat: true } } }
						: e
			)
		);
		expect(defaultLocalPick(view, MAC)?.model.id).toBe(LOADED_MODEL);
		const options = localModelOptions(view, DEVBOX).filter((o) =>
			o.group?.startsWith("Ollama")
		);
		expect(options.every((o) => o.disabled === true && o.warn === true)).toBe(true);
		expect(options[0]?.detail).toMatch(/Anthropic Messages API/);
	});

	it("saves a new default in the device settings", async () => {
		const view = await load();
		const pick = defaultLocalPick(view, DEVBOX)!;
		const kept = deviceSettingsOf(view, DEVBOX);
		const saved = withDefaultLocalModel(
			{ ...kept, local_models: { ...kept.local_models, context: 32_768 } },
			pick
		);
		expect(saved.local_models).toMatchObject({
			context: 32_768,
			default_model: { server: MAC_SERVER, model: LOADED_MODEL },
		});
		expect(saved.workspace_pool).toEqual(kept.workspace_pool);
		expect(withDefaultLocalModel(saved, null).local_models?.default_model).toBeNull();
	});
});

describe("the Local entry in the agent picker", () => {
	it("comes last among the device's logins, as Local with its default model", async () => {
		const view = await load();
		const options = accountOptions(view, MAC, NOW);
		const local = options.at(-1)!;
		expect(local).toMatchObject({
			value: MAC_LOCAL,
			label: "Local",
			detail: "Qwen3.6-35B-A3B on MacBook Pro",
		});
		expect(options.filter((o) => o.label === "Local")).toHaveLength(1);
		expect(options.some((o) => o.label.includes("Claude") && o.value === MAC_LOCAL)).toBe(
			false
		);
	});

	it("stands for the picked local login, else the default pick's", async () => {
		const view = await load();
		expect(localAccountOption(view, DEVBOX)?.value).toBe(DEVBOX_LOCAL);
		expect(localAccountOption(view, DEVBOX, OLLAMA_LOCAL)?.value).toBe(OLLAMA_LOCAL);
		expect(localAccountOption(view, DEVBOX, "acct-other")?.value).toBe(DEVBOX_LOCAL);
	});

	it("asks for a download on a Mac with no local model yet", async () => {
		const view = await edited((entities) =>
			entities.filter((e) => !(e.entity === "account" && e.login === "model_server"))
		);
		expect(localAccountOption(view, MAC)).toMatchObject({
			value: LOCAL_SETUP_VALUE,
			label: "Local",
			disabled: true,
		});
		// A device without a GPU and without a server URL has no Local entry.
		expect(localAccountOption(view, DEVBOX)).toBeNull();
	});

	it("groups the models by server and marks an unencrypted one", async () => {
		const view = await load();
		const options = localModelOptions(view, DEVBOX);
		expect(options.map((o) => [o.group, o.label])).toEqual([
			["Ollama on gpu-box (not encrypted)", "qwen3-coder:30b"],
			["Ollama on gpu-box (not encrypted)", "gpt-oss-20b"],
			["MacBook Pro", "Qwen3.6-35B-A3B"],
			["MacBook Pro", "gpt-oss-20b"],
			["MacBook Pro", "Ornith-1.5-9B"],
		]);
		const qwen = options.find((o) => o.value === localModelKey(DEVBOX_LOCAL, LOADED_MODEL))!;
		expect(qwen.detail).toBe("The default, Loaded");
		expect(options.every((o) => !o.disabled)).toBe(true);
	});

	it("offers a local login's server models in the model list", async () => {
		const view = await load();
		const models = modelsOf(view, view.tables.account.get(MAC_LOCAL));
		expect(models.map((m) => [m.id, m.default])).toEqual([
			[LOADED_MODEL, true],
			["gpt-oss-20b@mxfp4", false],
			["ornith-1.5-9b@q4_k_m", false],
		]);
		expect(models.every((m) => (m.efforts ?? []).length === 0)).toBe(true);
	});
});

describe("picked values", () => {
	it("names a login and a model in one value and reads it back", () => {
		const key = localModelKey(OLLAMA_LOCAL, "qwen3-coder:30b");
		expect(parseLocalModelKey(key)).toEqual({
			account: OLLAMA_LOCAL,
			model: "qwen3-coder:30b",
		});
		expect(modelChoice(key)).toEqual({ account: OLLAMA_LOCAL, model: key });
		expect(modelChoice("claude-opus-4")).toEqual({ model: "claude-opus-4" });
		expect(modelChoice("")).toEqual({ model: null });
		for (const value of [null, undefined, "", "local|", "local|acct|", "local||m", "claude"]) {
			expect(parseLocalModelKey(value)).toBeNull();
		}
	});

	it("lands a pick on its login and model, or on the default", async () => {
		const view = await load();
		const devbox = view.tables.account.get(DEVBOX_LOCAL)!;
		const ollama = resolveLocal(
			view,
			DEVBOX,
			devbox,
			localModelKey(OLLAMA_LOCAL, "qwen3-coder:30b")
		);
		expect([ollama.account.id, ollama.model]).toEqual([OLLAMA_LOCAL, "qwen3-coder:30b"]);
		const plain = resolveLocal(view, DEVBOX, devbox, "gpt-oss-20b@mxfp4");
		expect([plain.account.id, plain.model]).toEqual([DEVBOX_LOCAL, "gpt-oss-20b@mxfp4"]);
		const missing = resolveLocal(view, DEVBOX, devbox, localModelKey(OLLAMA_LOCAL, "gone"));
		expect([missing.account.id, missing.model]).toEqual([DEVBOX_LOCAL, LOADED_MODEL]);
		const fresh = resolveLocal(view, DEVBOX, view.tables.account.get(OLLAMA_LOCAL)!, null);
		expect([fresh.account.id, fresh.model]).toEqual([OLLAMA_LOCAL, "qwen3-coder:30b"]);
		expect(shownLocalModel(devbox, LOADED_MODEL)).toBe(
			localModelKey(DEVBOX_LOCAL, LOADED_MODEL)
		);
		expect(shownLocalModel(devbox, null)).toBeNull();
	});
});

describe("a local agent's name", () => {
	it("is Local with its model, never Claude", async () => {
		const view = await load();
		for (const id of [MAC_LOCAL_AGENT, DEVBOX_LOCAL_AGENT]) {
			const running = view.tables.agent.get(id)!;
			expect(agentKindLabel(view, running)).toBe("Local · Qwen3.6-35B-A3B");
			expect(agentCatalogModel(view, running)?.id).toBe(LOADED_CATALOG_MODEL);
		}
		const claude = [...view.tables.agent.values()].find(
			(a) =>
				a.vendor === "claude" &&
				a.account !== null &&
				a.local == null &&
				!a.account.includes("local")
		)!;
		expect(agentKindLabel(view, claude)).toBe("Claude");
	});

	it("names a model by the catalog, the server or its id", async () => {
		const view = await load();
		const on = (model: string | null, account: string | null = OLLAMA_LOCAL) =>
			localModelName(view, agent({ id: "x", account, model }));
		expect(on("gpt-oss:20b")).toBe("gpt-oss-20b");
		expect(on("qwen3-coder:30b")).toBe("qwen3-coder:30b");
		expect(on(DOWNLOADING_MODEL, null)).toBe("Qwen3.8-27B");
		expect(on("mystery", null)).toBe("mystery");
		expect(on(null)).toBeNull();
		expect(agentKindLabel(view, agent({ id: "y", account: OLLAMA_LOCAL, model: null }))).toBe(
			"Local"
		);
	});

	it("notes a server whose traffic isn't encrypted", async () => {
		const view = await load();
		expect(serverNote(view.tables.model_server.get(OLLAMA_SERVER)!)).toBe(
			"Not encrypted on the network"
		);
		expect(serverNote(view.tables.model_server.get(MAC_SERVER)!)).toBeNull();
	});
});
