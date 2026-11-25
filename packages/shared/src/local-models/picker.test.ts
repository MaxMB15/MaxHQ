// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity } from "../bindings";
import {
	DEVBOX_LOCAL,
	DOWNLOADING_MODEL,
	LOADED_MODEL,
	MAC_LOCAL,
	OLLAMA_LOCAL,
	OPENROUTER_ACCOUNT,
	OPENROUTER_MODEL,
} from "../data/mock-local-models";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import {
	agentOptions,
	filterModelOptions,
	localPickerOptions,
	localModelTag,
	modelOptionCount,
	opensLocalSetup,
	optionModel,
} from "./picker";
import { defaultLocalPick, localPicksOn } from "./models";
import { LOCAL_SETUP_VALUE, localModelKey, modelChoice } from "./select";

/** The sample view, with its entities changed by `edit`. */
const load = async (edit: (entities: Entity[]) => Entity[] = (e) => e): Promise<ViewState> => {
	const snapshot = await mockSource({ now: () => NOW, tick: false }).snapshot();
	return applySnapshot({ ...snapshot, entities: edit(snapshot.entities) });
};

/** Without any local login, as on a device with no model ready. */
const noLocalLogins = (entities: Entity[]): Entity[] =>
	entities.filter((e) => !(e.entity === "account" && e.login === "model_server"));

describe("the agent picker", () => {
	it("lists Local once, after the logins, then each cloud service as an agent of its own", async () => {
		const view = await load();
		const options = agentOptions(view, MAC, NOW);
		expect(options.at(-2)).toMatchObject({ value: MAC_LOCAL, label: "Local", mark: "local" });
		expect(options.at(-1)).toMatchObject({
			value: OPENROUTER_ACCOUNT,
			label: "OpenRouter",
			mark: "cloud",
		});
		expect(options.filter((o) => o.label === "Local")).toHaveLength(1);
		expect(options.slice(0, -2).map((o) => o.label)).not.toContain("Local");
		expect(options.slice(0, -2).every((o) => o.mark !== undefined)).toBe(true);
		expect(agentOptions(view, DEVBOX, NOW, OLLAMA_LOCAL).at(-1)?.value).toBe(OLLAMA_LOCAL);
	});

	it("keeps Local with its setup text when only a cloud service is set up", async () => {
		const view = await load((entities) =>
			entities.filter((e) => !(e.entity === "account" && e.id === MAC_LOCAL))
		);
		const options = agentOptions(view, MAC, NOW);
		expect(options.at(-2)).toMatchObject({
			value: LOCAL_SETUP_VALUE,
			label: "Local",
			detail: "Download a model in Settings first",
		});
		expect(options.at(-1)?.value).toBe(OPENROUTER_ACCOUNT);
	});

	it("still lists Local without a model, and leads to its setup", async () => {
		const view = await load(noLocalLogins);
		expect(agentOptions(view, MAC, NOW).at(-1)).toEqual({
			value: LOCAL_SETUP_VALUE,
			label: "Local",
			detail: "Download a model in Settings first",
			warn: true,
			mark: "local",
		});
		expect(agentOptions(view, DEVBOX, NOW).at(-1)).toMatchObject({
			value: LOCAL_SETUP_VALUE,
			detail: "Add a server URL in Settings first",
		});
	});
});

describe("the local model picker", () => {
	it("lists a cloud service's own models alone, which its search finds", async () => {
		const view = await load();
		const options = localPickerOptions(view, MAC, OPENROUTER_ACCOUNT);
		expect(options.map((o) => [o.group, o.label])).toEqual([
			["OpenRouter", "Z.ai: GLM 5.3 Flash"],
			["OpenRouter", "DeepSeek: DeepSeek V3.2"],
			["OpenRouter", "MoonshotAI: Kimi K2.5"],
			["OpenRouter", "Qwen: Qwen3 Coder 480B A35B"],
		]);
		expect(modelChoice(options[0]!.value)).toEqual({
			account: OPENROUTER_ACCOUNT,
			model: localModelKey(OPENROUTER_ACCOUNT, OPENROUTER_MODEL),
		});
		expect(filterModelOptions(options, "glm").map((o) => o.label)).toEqual([
			"Z.ai: GLM 5.3 Flash",
		]);
		expect(filterModelOptions(options, "z-ai/glm").map((o) => o.label)).toEqual([
			"Z.ai: GLM 5.3 Flash",
		]);
		// Local's own list leaves it out.
		expect(localPickerOptions(view, MAC).some((o) => o.group === "OpenRouter")).toBe(false);
	});

	it("groups the Mac's models under This Mac, each with its fit", async () => {
		const view = await load();
		const options = localPickerOptions(view, MAC);
		expect(options.map((o) => [o.group, o.label, o.tag])).toEqual([
			["This Mac", "Qwen3.6-35B-A3B", "Fits"],
			["This Mac", "gpt-oss-20b", "Fits"],
			["This Mac", "Ornith-1.5-9B", "Fits"],
			["This Mac", "Ornith-1.5-35B-A3B", "Not downloaded"],
			["This Mac", "Qwen3.8-27B", "Not downloaded"],
			["This Mac", "Qwen3-Coder-Next-80B-A3B", "Not downloaded"],
			[undefined, "Set up local models…", undefined],
		]);
		expect(options[0]).toMatchObject({
			value: localModelKey(MAC_LOCAL, LOADED_MODEL),
			detail: "The default, Loaded",
			tone: "good",
		});
		expect(options.find((o) => o.label === "Qwen3.8-27B")?.detail).toBe("Downloading, 41%");
	});

	it("puts a server at a URL under its own name, without a fit", async () => {
		const view = await load();
		const options = localPickerOptions(view, DEVBOX);
		const ollama = options.filter((o) => o.group === "Ollama on gpu-box (not encrypted)");
		expect(ollama.map((o) => o.label)).toEqual(["qwen3-coder:30b", "gpt-oss-20b"]);
		expect(ollama.every((o) => o.tag === undefined)).toBe(true);
		// devbox's own login on the Mac's server lists the Mac's models, and its catalog.
		expect(
			options.filter((o) => o.value.startsWith(localModelKey(DEVBOX_LOCAL, ""))).length
		).toBe(3);
		expect(options.at(-1)?.value).toBe(LOCAL_SETUP_VALUE);
	});

	it("shows a Mac's models turned off where its switch keeps them to itself", async () => {
		const view = await load((entities) =>
			entities.map((e) =>
				e.entity === "device_settings" && e.device === MAC
					? { ...e, local_models: { ...e.local_models, linked_devices: false } }
					: e
			)
		);
		const options = localPickerOptions(view, DEVBOX);
		const mac = options.filter((o) => o.group === "This Mac");
		expect(mac.map((o) => [o.label, o.disabled, o.detail])).toEqual([
			["Qwen3.6-35B-A3B", true, "Turned off on Mac"],
			["gpt-oss-20b", true, "Turned off on Mac"],
			["Ornith-1.5-9B", true, "Turned off on Mac"],
			[
				"Open Mac's Local models settings…",
				undefined,
				'Turn on "Linked devices can use this Mac\'s models" there',
			],
		]);
		// No fit tags, nor the Mac's catalog to download from.
		expect(mac.every((o) => o.tag === undefined)).toBe(true);
		expect(mac.every((o) => o.warn)).toBe(true);
		// The last row opens the Mac's page, where the switch is.
		expect(opensLocalSetup(mac.at(-1)!.value, DEVBOX)).toEqual({ device: MAC });
		// devbox's default moves off the Mac; the Mac's own picker stays as it was.
		expect(defaultLocalPick(view, DEVBOX)?.account.id).toBe(OLLAMA_LOCAL);
		expect(localPicksOn(view, DEVBOX).some((p) => p.account.id === DEVBOX_LOCAL)).toBe(false);
		expect(localPickerOptions(view, MAC).filter((o) => o.disabled)).toEqual([]);
	});

	it("tags a model too large for the GPU", async () => {
		const view = await load((entities) =>
			entities.map((e) =>
				e.entity === "local_models" && e.device === MAC && e.hardware.gpu
					? {
							...e,
							hardware: {
								...e.hardware,
								gpu: { ...e.hardware.gpu, budget: 1024 ** 3 },
							},
						}
					: e
			)
		);
		const account = view.tables.account.get(MAC_LOCAL)!;
		expect(localModelTag(view, account, LOADED_MODEL)).toEqual({
			label: "Too large",
			tone: "bad",
		});
		expect(localModelTag(view, account, "gone")).toBeNull();
	});
});

describe("picked values", () => {
	it("tell a setup row from a model", () => {
		expect(opensLocalSetup(LOCAL_SETUP_VALUE, MAC)).toEqual({ device: MAC });
		expect(opensLocalSetup(LOCAL_SETUP_VALUE, null)).toEqual({ device: null });
		expect(opensLocalSetup(`${LOCAL_SETUP_VALUE}|${DEVBOX}|${DOWNLOADING_MODEL}`, MAC)).toEqual(
			{
				device: DEVBOX,
			}
		);
		const model = localModelKey(MAC_LOCAL, LOADED_MODEL);
		expect(opensLocalSetup(model, MAC)).toBeNull();
		expect(modelChoice(model)).toEqual({ account: MAC_LOCAL, model });
		expect(opensLocalSetup(MAC_LOCAL, MAC)).toBeNull();
	});
});

describe("the model search", () => {
	it("narrows the rows by name or model ID, keeping the setup row", async () => {
		const view = await load();
		const options = localPickerOptions(view, MAC);
		const labels = (query: string) => filterModelOptions(options, query).map((o) => o.label);
		expect(labels("qwen 27b")).toEqual(["Qwen3.8-27B", "Set up local models…"]);
		// A local model's ID, past its account in the value.
		expect(labels("ud-q4_k_xl")).toEqual(["Qwen3.6-35B-A3B", "Set up local models…"]);
		expect(labels("nothing like it")).toEqual(["Set up local models…"]);
		expect(labels("")).toEqual(options.map((o) => o.label));
	});

	it("counts every row but the setup row", async () => {
		const view = await load();
		const options = localPickerOptions(view, MAC);
		expect(modelOptionCount(options)).toBe(options.length - 1);
		expect(modelOptionCount([{ value: "opus", label: "Opus 4.8" }])).toBe(1);
	});

	it("reads the model out of each kind of row", () => {
		expect(optionModel(localModelKey(MAC_LOCAL, LOADED_MODEL))).toBe(LOADED_MODEL);
		expect(optionModel(`${LOCAL_SETUP_VALUE}|${DEVBOX}|${DOWNLOADING_MODEL}`)).toBe(
			DOWNLOADING_MODEL
		);
		expect(optionModel(`${LOCAL_SETUP_VALUE}|${MAC}`)).toBe("");
		expect(optionModel(LOCAL_SETUP_VALUE)).toBe("");
		expect(optionModel("anthropic/z-ai/glm-5.3-flash[1m]")).toBe(
			"anthropic/z-ai/glm-5.3-flash[1m]"
		);
	});
});
