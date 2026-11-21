// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The local models of the sample data. The Mac runs its own model server
// with Qwen3.6-35B-A3B loaded and a Local agent on it. devbox has no GPU, so
// it reaches the Mac's server over the maxd link, where a Local agent of its
// own runs, and an Ollama server at a URL on another box. The catalog offers
// six models, and the Mac is downloading one of them. Each call changes the
// sample entities the way `maxd` would publish the change, and a device
// whose link doesn't offer local models is refused the way the app does.

import type {
	Account,
	ActionOutcome,
	Agent,
	Capabilities,
	CatalogFile,
	CatalogModel,
	DeviceId,
	Entity,
	LocalModels,
	ModelAction,
	ModelFile,
	ModelLoad,
	ModelScore,
	ModelServer,
	ServerFlavor,
	ServerModel,
	ServerProbe,
	Timestamp,
} from "../bindings";
import { managedModelId } from "../local-models/models";
import { urlParts, type UrlParts } from "../local-models/wording";
import type { FleetContext } from "./mock-fleet";
import { DEVBOX, MAC, seed } from "./mock-view";
import type { ActionResult, DataSource } from "./source";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const GIB = 1024 ** 3;

/** The Mac's own model server. */
export const MAC_SERVER = "srv-mac-llama";
/** The Ollama server devbox reaches at a URL. */
export const OLLAMA_SERVER = "srv-devbox-ollama";
/** The Ollama server's URL, on a placeholder host. */
export const OLLAMA_URL = "http://gpu-box.example.com:11434";
/** The Mac's local account, on its own server. */
export const MAC_LOCAL = "acct-local-mac";
/** devbox's local account on the Mac's server, over the link. */
export const DEVBOX_LOCAL = "acct-local-devbox-mac";
/** devbox's local account on the Ollama server. */
export const OLLAMA_LOCAL = "acct-local-devbox-ollama";
/** The catalog model the Mac has loaded, the catalog's default. */
export const LOADED_CATALOG_MODEL = "qwen3.6-35b-a3b";
/** Its ID on the Mac's managed server, from the file it loaded. */
export const LOADED_MODEL = "qwen3.6-35b-a3b@ud-q4_k_xl";
/** The model the Mac is downloading. */
export const DOWNLOADING_MODEL = "qwen3.8-27b";
/** The Mac's Local agent. */
export const MAC_LOCAL_AGENT = "agent-local-parser";
/** devbox's Local agent, on the Mac's server. */
export const DEVBOX_LOCAL_AGENT = "agent-local-changelog";

/** The context size the sample servers load models with. */
const CONTEXT = 65_536;

/** A made-up hex digest of `length` characters, plainly not a real one. */
const digest = (n: number, length: 40 | 64): string =>
	n
		.toString(16)
		.padStart(8, "0")
		.repeat(length / 8);

const file = (
	name: string,
	quant: string,
	bytes: number,
	n: number,
	recommended = false
): CatalogFile => ({ name, quant, bytes, sha256: digest(n, 64), recommended });

const score = (
	benchmark: string,
	value: number,
	harness: string | null,
	url: string,
	date: string
): ModelScore => ({ benchmark, value, harness, source: "vendor", url, date });

/** The six catalog models, best pick first. */
export const mockCatalog = (): CatalogModel[] => [
	{
		id: "qwen3.6-35b-a3b",
		name: "Qwen3.6-35B-A3B",
		repo: "unsloth/Qwen3.6-35B-A3B-GGUF",
		commit: digest(0x5a1c3e01, 40),
		license: "Apache-2.0",
		released: "2026-04-16",
		params_b: 35,
		active_params_b: 3,
		files: [
			file("Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf", "UD-Q4_K_XL", 22_400_000_000, 0x0a01, true),
			file("Qwen3.6-35B-A3B-IQ4_XS.gguf", "IQ4_XS", 17_700_000_000, 0x0a02),
		],
		kv: { full_layers: 10, kv_heads: 2, head_dim: 256 },
		native_context: 262_144,
		notes: "Fast mixture of experts with a small KV cache: the all-round pick from 32 GB up. Take IQ4_XS when the Mac is busy.",
		scores: [
			score(
				"SWE-bench Verified",
				73.4,
				null,
				"https://huggingface.co/Qwen/Qwen3.6-35B-A3B",
				"2026-04-16"
			),
		],
	},
	{
		id: "ornith-1.5-35b-a3b",
		name: "Ornith-1.5-35B-A3B",
		repo: "ornith-ai/Ornith-1.5-35B-A3B-GGUF",
		commit: digest(0x5a1c3e02, 40),
		license: "MIT",
		released: "2026-08-18",
		params_b: 35,
		active_params_b: 3,
		files: [file("Ornith-1.5-35B-A3B-Q4_K_M.gguf", "Q4_K_M", 21_700_000_000, 0x0b01, true)],
		kv: { full_layers: 10, kv_heads: 2, head_dim: 256 },
		native_context: 262_144,
		notes: "A coding finetune of the same mixture of experts, tuned in Claude Code. Strongest of its size on agent work.",
		scores: [
			score(
				"SWE-bench Verified",
				79,
				"Claude Code",
				"https://huggingface.co/ornith-ai/Ornith-1.5-35B-A3B",
				"2026-08-18"
			),
			score(
				"Terminal-Bench 2.1",
				68.5,
				"Claude Code",
				"https://huggingface.co/ornith-ai/Ornith-1.5-35B-A3B",
				"2026-08-18"
			),
		],
	},
	{
		id: DOWNLOADING_MODEL,
		name: "Qwen3.8-27B",
		repo: "unsloth/Qwen3.8-27B-GGUF",
		commit: digest(0x5a1c3e03, 40),
		license: "Apache-2.0",
		released: "2026-08-05",
		params_b: 27,
		active_params_b: null,
		files: [
			file("Qwen3.8-27B-UD-Q4_K_XL.gguf", "UD-Q4_K_XL", 17_700_000_000, 0x0c01, true),
			file("Qwen3.8-27B-Q8_0.gguf", "Q8_0", 29_000_000_000, 0x0c02),
		],
		kv: { full_layers: 16, kv_heads: 4, head_dim: 256 },
		native_context: 262_144,
		notes: "A dense model: better answers than the mixtures of experts, at a quarter of their speed.",
		scores: [
			score(
				"Terminal-Bench 2.1",
				73,
				"Claude Code",
				"https://huggingface.co/Qwen/Qwen3.8-27B",
				"2026-08-05"
			),
			score(
				"SWE-bench Pro",
				61.7,
				"Claude Code",
				"https://huggingface.co/Qwen/Qwen3.8-27B",
				"2026-08-05"
			),
		],
	},
	{
		id: "gpt-oss-20b",
		name: "gpt-oss-20b",
		repo: "ggml-org/gpt-oss-20b-GGUF",
		commit: digest(0x5a1c3e04, 40),
		license: "Apache-2.0",
		released: "2025-08-05",
		params_b: 21,
		active_params_b: 3.6,
		files: [file("gpt-oss-20b-mxfp4.gguf", "MXFP4", 12_100_000_000, 0x0d01, true)],
		kv: { full_layers: 12, kv_heads: 8, head_dim: 64, swa_layers: 12, swa_window: 128 },
		native_context: 131_072,
		notes: "Light and quick, and leaves room for Xcode. Weaker on long agent runs.",
		scores: [],
	},
	{
		id: "ornith-1.5-9b",
		name: "Ornith-1.5-9B",
		repo: "ornith-ai/Ornith-1.5-9B-GGUF",
		commit: digest(0x5a1c3e05, 40),
		license: "MIT",
		released: "2026-08-18",
		params_b: 9,
		active_params_b: null,
		files: [
			file("Ornith-1.5-9B-Q4_K_M.gguf", "Q4_K_M", 5_800_000_000, 0x0e01, true),
			file("Ornith-1.5-9B-Q8_0.gguf", "Q8_0", 9_800_000_000, 0x0e02),
		],
		kv: { full_layers: 8, kv_heads: 4, head_dim: 256 },
		native_context: 262_144,
		notes: "Small enough for any Mac with 16 GB. Good for small, well-described tasks.",
		scores: [
			score(
				"SWE-bench Verified",
				70.6,
				null,
				"https://huggingface.co/ornith-ai/Ornith-1.5-9B",
				"2026-08-18"
			),
		],
	},
	{
		id: "qwen3-coder-next-80b-a3b",
		name: "Qwen3-Coder-Next-80B-A3B",
		repo: "unsloth/Qwen3-Coder-Next-GGUF",
		commit: digest(0x5a1c3e06, 40),
		license: "Apache-2.0",
		released: "2026-02-03",
		params_b: 80,
		active_params_b: 3,
		files: [
			file("Qwen3-Coder-Next-UD-Q4_K_XL.gguf", "UD-Q4_K_XL", 46_000_000_000, 0x0f01, true),
		],
		kv: { full_layers: 12, kv_heads: 2, head_dim: 256 },
		native_context: 262_144,
		notes: "A large coding model for Macs with 64 GB or more.",
		scores: [],
	},
];

const catalogModel = (id: string): CatalogModel | undefined =>
	mockCatalog().find((m) => m.id === id);

/** Where each sample device keeps model files. */
const MODELS_DIR: Readonly<Record<DeviceId, string>> = {
	[MAC]: "/Users/me/.local/share/maxhq/models",
	[DEVBOX]: "/home/dev/.local/share/maxhq/models",
};

/** Where each sample device keeps its local accounts' Claude Code config, never the user's own. */
const LOCAL_CONFIG: Readonly<Record<DeviceId, string>> = {
	[MAC]: "/Users/me/.local/share/maxhq/local-claude",
	[DEVBOX]: "/home/dev/.local/share/maxhq/local-claude",
};

const modelPath = (device: DeviceId, model: CatalogModel, name: string): string =>
	`${MODELS_DIR[device] ?? ""}/${model.repo}/${name}`;

const fileState = (
	device: DeviceId,
	id: string,
	name: string,
	done: number | null,
	state: ModelFile["state"]
): ModelFile => {
	const model = catalogModel(id)!;
	const total = model.files.find((f) => f.name === name)!.bytes;
	return {
		model: id,
		file: name,
		path: modelPath(device, model, name),
		bytes_done: done ?? total,
		bytes_total: total,
		state,
	};
};

/** A catalog file on a managed server, with the ID `maxd` gives it. */
const managedModel = (id: string, name: string, load: ModelLoad): ServerModel => {
	const model = catalogModel(id);
	const quant = model?.files.find((f) => f.name === name)?.quant ?? name;
	return {
		id: managedModelId(id, quant),
		label: model?.name ?? id,
		catalog: { model: id, file: name },
		context: CONTEXT,
		load,
	};
};

const localAccount = (id: string, device: DeviceId, label: string, server: string): Account => ({
	id,
	device,
	vendor: "claude",
	label,
	login: "model_server",
	origin: null,
	config_dir: `${LOCAL_CONFIG[device] ?? ""}/${server}`,
	email: null,
	organization: null,
	plan: null,
	models: [],
	usage: null,
	usage_failure: null,
	model_server: server,
});

const probed = (at: Timestamp, flavor: ServerFlavor, context: number | null): ServerProbe => ({
	at,
	apis:
		flavor === "vllm"
			? { messages: false, chat: true, responses: true }
			: flavor === "lm_studio"
				? { messages: false, chat: true, responses: false }
				: { messages: true, chat: true, responses: flavor !== "llama_cpp" },
	context,
	error: null,
});

const servers = (now: Timestamp): ModelServer[] => [
	{
		id: MAC_SERVER,
		device: MAC,
		kind: { type: "managed" },
		label: "MacBook Pro",
		base_url: null,
		probe: probed(now - 40 * MIN, "llama_cpp", CONTEXT),
		models: [
			managedModel(LOADED_CATALOG_MODEL, "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf", {
				state: "loaded",
				slots: 1,
				context: CONTEXT,
				since: now - 40 * MIN,
			}),
			managedModel("gpt-oss-20b", "gpt-oss-20b-mxfp4.gguf", { state: "unloaded" }),
			managedModel("ornith-1.5-9b", "Ornith-1.5-9B-Q4_K_M.gguf", { state: "unloaded" }),
		],
	},
	{
		id: OLLAMA_SERVER,
		device: DEVBOX,
		kind: { type: "external", flavor: "ollama" },
		label: "Ollama on gpu-box",
		base_url: OLLAMA_URL,
		token: false,
		probe: probed(now - 2 * HOUR, "ollama", null),
		models: [
			{
				id: "qwen3-coder:30b",
				label: "qwen3-coder:30b",
				catalog: null,
				context: null,
				load: null,
			},
			{
				id: "gpt-oss:20b",
				label: "gpt-oss:20b",
				catalog: { model: "gpt-oss-20b", file: "gpt-oss-20b-mxfp4.gguf" },
				context: null,
				load: null,
			},
		],
	},
];

const localModels = (now: Timestamp): LocalModels[] => [
	{
		device: MAC,
		hardware: {
			chip: "Apple M4 Max",
			gpu_cores: 32,
			ram: 36 * GIB,
			gpu: { budget: 27 * GIB, source: "metal" },
		},
		models_dir: MODELS_DIR[MAC]!,
		disk_free: 212 * GIB,
		runtime: { support: "managed", version: "b7412", state: { type: "idle" } },
		catalog: { updated_at: now - 6 * DAY, models: mockCatalog() },
		files: [
			fileState(MAC, LOADED_CATALOG_MODEL, "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf", null, {
				state: "ready",
			}),
			fileState(MAC, "gpt-oss-20b", "gpt-oss-20b-mxfp4.gguf", null, { state: "ready" }),
			fileState(MAC, "ornith-1.5-9b", "Ornith-1.5-9B-Q4_K_M.gguf", null, { state: "ready" }),
			fileState(MAC, DOWNLOADING_MODEL, "Qwen3.8-27B-UD-Q4_K_XL.gguf", 7_260_000_000, {
				state: "downloading",
				phase: "downloading",
			}),
		],
	},
	{
		device: DEVBOX,
		hardware: { chip: null, gpu_cores: null, ram: 64 * GIB, gpu: null },
		models_dir: MODELS_DIR[DEVBOX]!,
		disk_free: 410 * GIB,
		runtime: { support: "no_gpu", version: null, state: { type: "idle" } },
		catalog: { updated_at: now - 6 * DAY, models: mockCatalog() },
		files: [],
	},
];

const accounts = (): Account[] => [
	localAccount(MAC_LOCAL, MAC, "MacBook Pro", MAC_SERVER),
	localAccount(DEVBOX_LOCAL, DEVBOX, "MacBook Pro", MAC_SERVER),
	localAccount(OLLAMA_LOCAL, DEVBOX, "Ollama on gpu-box", OLLAMA_SERVER),
];

const agents = (now: Timestamp): Agent[] => [
	seed(now, {
		id: MAC_LOCAL_AGENT,
		title: "Parser edge-case tests",
		workspace: "ws-mac-comet",
		account: MAC_LOCAL,
		model: LOADED_MODEL,
		control: "held",
		pid: 52_310,
		activity: { state: "busy" },
		context: { used_tokens: 31_200, window_tokens: CONTEXT },
		local: {
			prompt_per_second: 1_180,
			decode_per_second: 74,
			context_used: 31_200,
			context_size: CONTEXT,
		},
		started_at: now - 25 * MIN,
		last_activity_at: now - 3_000,
	}),
	seed(now, {
		id: DEVBOX_LOCAL_AGENT,
		title: "Changelog for the next release",
		workspace: "ws-bravo",
		account: DEVBOX_LOCAL,
		model: LOADED_MODEL,
		control: "held",
		pid: 9_144,
		activity: { state: "done_unread" },
		context: { used_tokens: 22_800, window_tokens: CONTEXT },
		local: {
			prompt_per_second: 1_020,
			decode_per_second: 61,
			context_used: 22_800,
			context_size: CONTEXT,
		},
		started_at: now - 2 * HOUR,
		last_activity_at: now - 9 * MIN,
	}),
];

/**
 * Every local model entity in the sample view: the two servers, what each
 * device knows, the three local accounts and the two Local agents.
 */
export const mockLocalEntities = (now: Timestamp): Entity[] => [
	...servers(now).map((v) => ({ entity: "model_server" as const, ...v })),
	...localModels(now).map((v) => ({ entity: "local_models" as const, ...v })),
	...accounts().map((v) => ({ entity: "account" as const, ...v })),
	...agents(now).map((v) => ({ entity: "agent" as const, ...v })),
];

/** The calls this module answers. */
export type LocalModelCalls = Pick<DataSource, "modelAction">;

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const ok = (data: ActionOutcome): ActionResult => ({ status: "ok", data });

const refused = (reason: string): ActionResult => ok({ status: "refused", reason });

const gone = (what: string): ActionResult => ({
	status: "error",
	error: { kind: "not_found", what },
});

const pause = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** The local model calls with nothing behind them, refused the way an older `maxd` does. */
export const noLocalModels: LocalModelCalls = {
	modelAction: async () => refused("This device doesn't offer it yet."),
};

/** What kind of server answers at a URL, from its port, as a probe would find it. */
const flavorOf = (url: UrlParts): ServerFlavor => {
	if (url.port === "11434" || url.host.includes("ollama")) return "ollama";
	if (url.port === "1234") return "lm_studio";
	if (url.port === "8000") return "vllm";
	return "llama_cpp";
};

/** A server URL's parts, or why it isn't one. */
const serverUrl = (url: string): UrlParts | string => {
	const parsed = urlParts(url);
	if (!parsed) return "That isn't a URL. Give one such as http://devbox:11434.";
	if (parsed.scheme !== "http" && parsed.scheme !== "https") {
		return "A server URL starts with http:// or https://.";
	}
	return parsed;
};

/** What a probed URL serves: one sample model, named the way its kind of server names them. */
const urlModels = (flavor: ServerFlavor): ServerModel[] => [
	{
		id: flavor === "ollama" ? "qwen3.6:35b" : "Qwen3.6-35B-A3B",
		label: flavor === "ollama" ? "qwen3.6:35b" : "Qwen3.6-35B-A3B",
		catalog: null,
		context: flavor === "ollama" ? null : CONTEXT,
		load: null,
	},
];

/** The local model calls over the sample entities. */
export const mockLocalModels = (context: FleetContext): LocalModelCalls => {
	const { now, change, stepMs } = context;
	const all = () => [...context.entities()];
	const running = new Set<string>();
	let added = 0;

	/** Why `device` can't take a local model call, or null when it can. */
	const lacks = (device: DeviceId): ActionResult | null => {
		const link = all().find((e) => e.entity === "device_link" && e.device === device);
		if (link?.entity !== "device_link") return gone("This device");
		const does = link.capabilities as Capabilities | null;
		if (does && does.local_models !== true) {
			return {
				status: "error",
				error: {
					kind: "failed",
					message: `${link.name}'s maxd is too old to run local models. Update it first`,
				},
			};
		}
		return null;
	};

	const localOf = (device: DeviceId): LocalModels | undefined => {
		const found = all().find((e) => e.entity === "local_models" && e.device === device);
		return found?.entity === "local_models" ? found : undefined;
	};

	const serverOn = (device: DeviceId, id: string): ModelServer | undefined => {
		const found = all().find((e) => e.entity === "model_server" && e.id === id);
		return found?.entity === "model_server" && found.device === device ? found : undefined;
	};

	const managedOn = (device: DeviceId): ModelServer | undefined => {
		const found = all().find(
			(e) => e.entity === "model_server" && e.device === device && e.kind.type === "managed"
		);
		return found?.entity === "model_server" ? found : undefined;
	};

	const contextOn = (device: DeviceId): number => {
		const found = all().find((e) => e.entity === "device_settings" && e.device === device);
		return (found?.entity === "device_settings" && found.local_models?.context) || CONTEXT;
	};

	const editServer = (id: string, edit: (server: ModelServer) => Partial<ModelServer>) =>
		change((base) =>
			base.map((e) => (e.entity === "model_server" && e.id === id ? { ...e, ...edit(e) } : e))
		);

	const editFiles = (device: DeviceId, edit: (files: ModelFile[]) => ModelFile[]) =>
		change((base) =>
			base.map((e) =>
				e.entity === "local_models" && e.device === device
					? { ...e, files: edit(e.files) }
					: e
			)
		);

	const fileOf = (device: DeviceId, model: string, name: string): ModelFile | undefined =>
		localOf(device)?.files.find((f) => f.model === model && f.file === name);

	const setFile = (device: DeviceId, model: string, name: string, patch: Partial<ModelFile>) =>
		editFiles(device, (files) =>
			files.map((f) => (f.model === model && f.file === name ? { ...f, ...patch } : f))
		);

	const setLoad = (server: string, model: string, load: ModelLoad) =>
		editServer(server, (s) => ({
			models: s.models.map((m) => (m.id === model ? { ...m, load } : m)),
		}));

	/** Probes a server after a moment, as `maxd` would. */
	const probe = async (id: string) => {
		await pause(stepMs);
		const found = all().find((e) => e.entity === "model_server" && e.id === id);
		if (found?.entity !== "model_server" || !found.base_url) return;
		const url = urlParts(found.base_url);
		if (!url) return;
		if (url.host.endsWith(".invalid")) {
			editServer(id, () => ({
				probe: {
					at: now(),
					apis: {},
					context: null,
					error: `Nothing answered at ${found.base_url}`,
				},
			}));
			return;
		}
		const flavor = flavorOf(url);
		editServer(id, () => ({
			kind: { type: "external", flavor },
			probe: probed(now(), flavor, flavor === "ollama" ? null : CONTEXT),
			models: urlModels(flavor),
		}));
	};

	/** Runs a download a quarter at a time, then checks it and adds it to the device's server. */
	const runDownload = async (device: DeviceId, model: string, name: string) => {
		const key = `${device}|${model}|${name}`;
		if (running.has(key)) return;
		running.add(key);
		try {
			for (;;) {
				await pause(stepMs);
				const current = fileOf(device, model, name);
				if (current?.state.state !== "downloading") return;
				const next = current.bytes_done + current.bytes_total / 4;
				if (next < current.bytes_total) {
					setFile(device, model, name, { bytes_done: next });
					continue;
				}
				setFile(device, model, name, {
					bytes_done: current.bytes_total,
					state: { state: "downloading", phase: "verifying" },
				});
				await pause(stepMs);
				if (fileOf(device, model, name)?.state.state !== "downloading") return;
				setFile(device, model, name, { state: { state: "ready" } });
				const server = managedOn(device);
				const served = (m: ServerModel) =>
					m.catalog?.model === model && m.catalog.file === name;
				if (server && !server.models.some(served)) {
					const entry: ServerModel = {
						...managedModel(model, name, { state: "unloaded" }),
						context: contextOn(device),
					};
					editServer(server.id, (s) => ({ models: [...s.models, entry] }));
				}
				return;
			}
		} finally {
			running.delete(key);
		}
	};

	const addServer = (
		device: DeviceId,
		label: string,
		url: string,
		token: string | null | undefined
	): ActionResult => {
		const parsed = serverUrl(url);
		if (typeof parsed === "string") return refused(parsed);
		added += 1;
		const id = `srv-${device}-${added}`;
		const server: ModelServer = {
			id,
			device,
			kind: { type: "external", flavor: "other" },
			label: label.trim() || (parsed.port ? `${parsed.host}:${parsed.port}` : parsed.host),
			base_url: url.trim().replace(/\/+$/, ""),
			token: Boolean(token),
			probe: null,
			models: [],
		};
		const account = localAccount(`acct-local-${id}`, device, server.label, id);
		change((base) => [
			...base,
			{ entity: "model_server", ...server },
			{ entity: "account", ...account },
		]);
		void probe(id);
		return ok({ status: "model_server_added", server: id });
	};

	const act = (device: DeviceId, action: ModelAction): ActionResult => {
		switch (action.action) {
			case "add_server":
				return addServer(device, action.label, action.url, action.token);
			case "probe_server": {
				// What answers at a URL, keeping nothing.
				const url = serverUrl(action.url);
				if (typeof url === "string") return refused(url);
				const flavor = url.host.endsWith(".invalid") ? "other" : flavorOf(url);
				const probe: ServerProbe =
					flavor === "other"
						? {
								at: now(),
								apis: {},
								context: null,
								error: `Nothing answered at ${action.url}`,
							}
						: probed(now(), flavor, flavor === "ollama" ? null : CONTEXT);
				const models = flavor === "other" ? [] : urlModels(flavor);
				return ok({ status: "server_probed", flavor, probe, models });
			}
			case "test_server": {
				const server = serverOn(device, action.server);
				if (!server) return gone("That server");
				if (server.kind.type === "managed") {
					editServer(server.id, (s) => ({
						probe: probed(now(), "llama_cpp", s.probe?.context ?? contextOn(device)),
					}));
				} else void probe(server.id);
				return DONE;
			}
			case "remove_server": {
				const server = serverOn(device, action.server);
				if (!server) return gone("That server");
				if (server.kind.type === "managed") {
					return refused("MaxHQ runs this server. Delete its models instead.");
				}
				change((base) =>
					base.filter(
						(e) =>
							!(e.entity === "model_server" && e.id === server.id) &&
							!(e.entity === "account" && e.model_server === server.id)
					)
				);
				return DONE;
			}
			case "download": {
				const local = localOf(device);
				if (!local) return gone("This device's models");
				if (local.runtime.support !== "managed") {
					return refused(
						"This device can't run models itself. Add a server URL instead."
					);
				}
				const model = local.catalog.models.find((m) => m.id === action.model);
				const wanted = model?.files.find((f) => f.name === action.file);
				if (!model || !wanted) return gone("That model");
				const held = fileOf(device, model.id, wanted.name);
				if (held?.state.state === "ready") return refused("It's downloaded already.");
				if (held) return refused("It's downloading already.");
				if (wanted.bytes > local.disk_free) {
					return refused("There isn't enough free disk space for it.");
				}
				editFiles(device, (files) => [
					...files,
					{
						model: model.id,
						file: wanted.name,
						path: `${local.models_dir}/${model.repo}/${wanted.name}`,
						bytes_done: 0,
						bytes_total: wanted.bytes,
						state: { state: "downloading", phase: "downloading" },
					},
				]);
				void runDownload(device, model.id, wanted.name);
				return DONE;
			}
			case "pause_download": {
				const held = fileOf(device, action.model, action.file);
				if (!held) return gone("That download");
				if (held.state.state !== "downloading") return refused("It isn't downloading.");
				setFile(device, action.model, action.file, { state: { state: "paused" } });
				return DONE;
			}
			case "resume_download": {
				const held = fileOf(device, action.model, action.file);
				if (!held) return gone("That download");
				if (held.state.state !== "paused" && held.state.state !== "failed") {
					return refused("It isn't paused.");
				}
				setFile(device, action.model, action.file, {
					state: { state: "downloading", phase: "downloading" },
				});
				void runDownload(device, action.model, action.file);
				return DONE;
			}
			case "delete_model": {
				const held = fileOf(device, action.model, action.file);
				if (!held) return gone("That model file");
				const server = managedOn(device);
				const served = server?.models.find(
					(m) => m.catalog?.model === action.model && m.catalog.file === action.file
				);
				const state = served?.load?.state;
				if (state === "loaded" || state === "loading") return refused("Unload it first.");
				editFiles(device, (files) =>
					files.filter((f) => !(f.model === held.model && f.file === held.file))
				);
				if (server && served) {
					editServer(server.id, (s) => ({
						models: s.models.filter((m) => m.id !== served.id),
					}));
				}
				return DONE;
			}
			case "load": {
				const server = serverOn(device, action.server);
				if (!server) return gone("That server");
				if (server.kind.type !== "managed")
					return refused("This server loads its own models.");
				const model = server.models.find((m) => m.id === action.model);
				if (!model) return gone("That model");
				if (model.load?.state === "loaded" || model.load?.state === "loading") return DONE;
				// One model at a time: loading another frees the memory of the one before.
				editServer(server.id, (s) => ({
					models: s.models.map((m) =>
						m.id === model.id
							? { ...m, load: { state: "loading" } }
							: m.load?.state === "loaded"
								? { ...m, load: { state: "unloaded" } }
								: m
					),
				}));
				// Halfway in memory, then serving.
				void pause(stepMs / 2)
					.then(() => setLoad(server.id, model.id, { state: "loading", percent: 50 }))
					.then(() => pause(stepMs / 2))
					.then(() =>
						setLoad(server.id, model.id, {
							state: "loaded",
							slots: 1,
							context: contextOn(device),
							since: now(),
						})
					);
				return DONE;
			}
			case "unload": {
				const server = serverOn(device, action.server);
				if (!server) return gone("That server");
				if (server.kind.type !== "managed")
					return refused("This server loads its own models.");
				if (!server.models.some((m) => m.id === action.model)) return gone("That model");
				setLoad(server.id, action.model, { state: "unloaded" });
				return DONE;
			}
		}
	};

	return {
		modelAction: async (device, action) => lacks(device) ?? act(device, action),
	};
};
