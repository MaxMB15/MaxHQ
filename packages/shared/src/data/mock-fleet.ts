// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The fleet calls of the sample data: load, diagnostics, tool and maxd
// updates, the Add device wizard and routes. Each one acts on the sample
// entities the way the backend would, with a short pause between steps.

import type { DeviceId, HelperTool, Timestamp, ToolPlace, ToolState } from "../bindings";
import type {
	ContainerUse,
	Entity,
	FleetStatus,
	HostLoad,
	ProcessUse,
	SetupHost,
	SetupProgress,
	SetupStep,
	ToolInfo,
	ToolTarget,
	UseBucket,
} from "../fleet/wire";
import { DEVBOX, MAC, MHQ_WORKSPACE } from "./mock-view";
import type { CommandResult, DataSource } from "./source";

/** What the fleet calls need from the sample source. */
export type FleetContext = {
	readonly now: () => Timestamp;
	/** The entities as the view has them now. */
	readonly entities: () => Iterable<Entity>;
	/** Changes the sample entities and sends the delta. */
	readonly change: (edit: (base: Entity[]) => Entity[]) => void;
	/** How long each step takes. */
	readonly stepMs: number;
};

const GIB = 1024 ** 3;

const OK: CommandResult = { status: "ok", data: null };

const failed = (message: string) => ({
	status: "error" as const,
	error: { kind: "failed" as const, message },
});

// The Mac is quiet and devbox carries its containers' agents, so new agents
// land on the Mac unless you pick otherwise.
const LOADS: Record<DeviceId, Omit<HostLoad, "sampled_at">> = {
	[MAC]: { cpu_util: 0.14, cpu_count: 12, mem_available: 14 * GIB, mem_total: 36 * GIB },
	[DEVBOX]: { cpu_util: 0.71, cpu_count: 16, mem_available: 18 * GIB, mem_total: 64 * GIB },
};

const HOST = { type: "host" } as const;

/** A tool MaxHQ installs, at `version` once it's in. */
const helper = (
	tool: HelperTool,
	place: ToolPlace,
	version: string | null,
	state: ToolState
): ToolInfo => ({
	kind: { type: "helper", tool },
	place,
	path: version ? `/opt/maxhq/tools/${tool}/${version}/bin/${tool}` : null,
	version,
	latest: version,
	owner: "max_hq",
	method: "maxhq",
	update: { type: "auto" },
	state,
});

/**
 * The agent CLIs on each device. The Mac's Claude Code is yours, so MaxHQ
 * only shows its command. devbox's host CLIs wait for a tick in settings,
 * and the workspace MaxHQ made updates its own.
 */
export const mockTools = (): Entity[] => [
	{
		entity: "device_tools",
		device: MAC,
		tools: [
			{
				kind: { type: "agent", tool: "claude" },
				place: HOST,
				path: "/Users/dev/.local/bin/claude",
				version: "2.1.40",
				latest: "2.1.42",
				owner: "user",
				method: "native",
				update: { type: "manual", command: "claude update" },
				state: { type: "idle" },
			},
			{
				kind: { type: "agent", tool: "codex" },
				place: HOST,
				path: "/opt/homebrew/bin/codex",
				version: "0.52.0",
				latest: "0.52.0",
				owner: "user",
				method: "brew",
				update: { type: "manual", command: "brew upgrade codex" },
				state: { type: "idle" },
			},
			{
				kind: { type: "agent", tool: "cursor" },
				place: HOST,
				path: "/Users/dev/.local/bin/agent",
				version: "2026.10.22-27dcf5f",
				latest: null,
				owner: "user",
				method: "native",
				update: { type: "never" },
				state: { type: "idle" },
			},
			helper("node", HOST, "22.11.0", { type: "idle" }),
			helper("jdk", HOST, null, {
				type: "running",
				message: "Downloading",
				phase: "downloading",
				bytes_done: 120_000_000,
				bytes_total: 190_000_000,
			}),
			helper("android_sdk", HOST, null, {
				type: "licences",
				pending: [
					{
						id: "android-sdk-license",
						text: "Terms and conditions for the Android Software Development Kit.",
						text_sha256:
							"0d6f4b8e2c1a9f7d5b3e1c0a8f6d4b2e0c9a7f5d3b1e9c7a5f3d1b0e8c6a4f2d",
					},
				],
			}),
			{
				...helper("node", HOST, "1.8.2", { type: "idle" }),
				kind: { type: "acp", id: "goose" },
				path: "/Users/dev/.local/share/maxhq/tools/goose/1.8.2/goose",
				latest: "1.9.0",
			},
		],
	},
	{
		entity: "device_tools",
		device: DEVBOX,
		tools: [
			{
				kind: { type: "agent", tool: "claude" },
				place: HOST,
				path: "/home/dev/.local/bin/claude",
				version: "2.1.38",
				latest: "2.1.42",
				owner: "user",
				method: "native",
				update: { type: "needs_tick" },
				state: { type: "idle" },
			},
			{
				kind: { type: "agent", tool: "claude" },
				place: { type: "workspace", workspace: MHQ_WORKSPACE },
				path: "/home/agent/.local/bin/claude",
				version: "2.1.41",
				latest: "2.1.42",
				owner: "max_hq",
				method: "native",
				update: { type: "auto" },
				state: { type: "idle" },
			},
			{
				kind: { type: "agent", tool: "codex" },
				place: { type: "workspace", workspace: MHQ_WORKSPACE },
				path: "/home/agent/.local/bin/codex",
				version: "0.52.0",
				latest: "0.52.0",
				owner: "max_hq",
				method: "npm",
				update: { type: "auto" },
				state: { type: "idle" },
			},
			helper("gh", HOST, "2.63.0", { type: "idle" }),
			helper("uv", HOST, null, {
				type: "failed",
				reason: "The download's checksum didn't match",
			}),
			helper("node", { type: "workspace", workspace: MHQ_WORKSPACE }, null, {
				type: "running",
				message: "Checking the download",
				phase: "verifying",
			}),
		],
	},
];

const samePlace = (a: ToolInfo, b: ToolTarget) =>
	a.kind.type === "agent" &&
	a.kind.tool === b.tool &&
	a.place.type === b.place.type &&
	(a.place.type === "host" ||
		(b.place.type === "workspace" && a.place.workspace === b.place.workspace));

/** The hosts the Add device wizard offers. */
const SETUP_HOSTS: readonly SetupHost[] = [
	{ host: "devbox", from: "ssh_config", address: "devbox.tail1234.ts.net", link: "link-devbox" },
	{ host: "spare-box", from: "tailnet", address: "spare-box.tail1234.ts.net", link: null },
	{ host: "build-box", from: "ssh_config", address: "192.168.1.60", link: null },
];

/** The fingerprint each sample host shows. */
const FINGERPRINT = "SHA256:q3Vt0bYx9mJ2kZ8hR4fWc1nP7sLd6uEa5oGi0XyTz4M";

const pause = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** A wave that stays between 0 and 1, so the history looks lived in. */
const wave = (seed: number, at: number) =>
	0.5 + 0.35 * Math.sin(at / 9 + seed) + 0.15 * Math.sin(at / 2.3 + seed * 2);

const deviceOfWorkspace = (entities: Entity[], workspace: string): DeviceId | null => {
	const found = entities.find((e) => e.entity === "workspace" && e.id === workspace);
	return found?.entity === "workspace" ? found.device : null;
};

/** The fleet calls over the sample entities. */
export const mockFleet = (
	context: FleetContext
): Pick<
	DataSource,
	| "readLoad"
	| "readDiagnostics"
	| "updateTools"
	| "updateFleet"
	| "setupDevice"
	| "confirmHostKey"
	| "setRoutes"
	| "listSetupHosts"
> => {
	const { now, change, stepMs } = context;
	const all = () => [...context.entities()];
	const linkOf = (device: DeviceId) =>
		all().find((e) => e.entity === "device_link" && e.device === device);
	const connected = (device: DeviceId) => {
		const link = linkOf(device);
		return link?.entity === "device_link" && link.state.state === "connected";
	};
	const known = (device: DeviceId) => all().some((e) => e.entity === "device" && e.id === device);
	// The setups waiting for their host key, by ID.
	const asking = new Map<string, (ok: boolean) => void>();
	const hosts = SETUP_HOSTS.map((h) => ({ ...h }));
	let setups = 0;

	const diagnostics = (device: DeviceId, windowSecs: number, bucketSecs: number) => {
		const at = now();
		const entities = all();
		const seed = device === MAC ? 1 : 4;
		const scale = device === MAC ? 0.3 : 1;
		const count = Math.max(1, Math.floor(windowSecs / bucketSecs));
		const buckets: UseBucket[] = Array.from({ length: count }, (_, i) => {
			const avg = Math.round(100 * scale * wave(seed, i) * 0.9);
			const started = at - (count - i) * bucketSecs * 1000;
			return {
				started_at: started,
				ended_at: started + bucketSecs * 1000,
				avg_cpu: avg,
				max_cpu: Math.min(100, avg + Math.round(18 * wave(seed + 2, i))),
				max_rss: (900 + 120 * i) * 1024 * 1024,
				max_processes: 8,
			};
		});
		const agents = entities.flatMap((e) =>
			e.entity === "agent" &&
			e.parent === null &&
			deviceOfWorkspace(entities, e.workspace) === device &&
			e.activity.state !== "not_accessible"
				? [e]
				: []
		);
		const top: ProcessUse[] = agents.slice(0, 5).map((agent, i) => {
			const busy = agent.activity.state.startsWith("busy");
			const cpuNow = busy ? 62 - i * 9 : 1.5 + i;
			return {
				agent: agent.id,
				workspace: agent.workspace,
				pid: 41200 + i * 37,
				command:
					agent.vendor === "claude"
						? "claude -p --output-format stream-json --verbose"
						: "codex app-server",
				cpu_now: cpuNow,
				cpu_avg: Math.max(0.4, cpuNow * 0.6),
				cpu_max: Math.min(100, cpuNow * 1.6 + 8),
				rss_now: (380 + i * 90) * 1024 * 1024,
				rss_max: (520 + i * 110) * 1024 * 1024,
			};
		});
		top.push({
			agent: null,
			workspace: null,
			pid: 812,
			command: "maxd serve",
			cpu_now: 0.8,
			cpu_avg: 0.6,
			cpu_max: 3.1,
			rss_now: 46 * 1024 * 1024,
			rss_max: 58 * 1024 * 1024,
		});
		const containers: ContainerUse[] = entities.flatMap((e, i) =>
			e.entity === "workspace" && e.device === device && e.location.type === "container"
				? [
						{
							workspace: e.id,
							cpu_ns: (40 + (i % 4) * 70) * 1e9,
							memory: (1.2 + (i % 3) * 0.9) * GIB,
						},
					]
				: []
		);
		return { buckets, top, containers, error: null };
	};

	const fleetStep = async (
		device: DeviceId,
		tools: boolean,
		onProgress: (progress: { device: DeviceId; status: FleetStatus }) => void
	) => {
		const send = (status: FleetStatus) => onProgress({ device, status });
		const record = all().find((e) => e.entity === "device" && e.id === device);
		if (record?.entity !== "device")
			return send({ status: "failed", reason: "unknown device" });
		if (!connected(device)) return send({ status: "skipped", reason: "offline" });
		if (record.winding_down_until !== null && record.winding_down_until > now()) {
			return send({ status: "skipped", reason: "winding down" });
		}
		if (device === MAC) {
			return send({ status: "skipped", reason: "this Mac's maxd runs from a dev tree" });
		}
		if (tools) {
			send({ status: "tools" });
			await pause(stepMs);
		}
		send({ status: "uploading" });
		await pause(stepMs);
		send({ status: "restarting" });
		await pause(stepMs);
		if (record.workspace_host !== null) {
			send({ status: "guests" });
			await pause(stepMs);
		}
		send({ status: "done" });
	};

	const setup = async (
		host: string,
		onProgress: (progress: SetupProgress) => void
	): Promise<
		| { status: "ok"; data: string }
		| { status: "error"; error: ReturnType<typeof failed>["error"] }
	> => {
		setups += 1;
		const id = `setup-${setups}`;
		const step = async (which: SetupStep, message: string) => {
			onProgress({ type: "step", setup: id, step: which, message });
			await pause(stepMs);
		};
		const stop = (which: SetupStep, reason: string) => {
			onProgress({ type: "failed", setup: id, step: which, reason });
			return failed(reason);
		};
		const found = hosts.find((h) => h.host === host);
		if (found?.link) return stop("pick", `${host} is already set up`);
		await step("host_key", `Reading ${host}'s host key`);
		const ok = await new Promise<boolean>((resolve) => {
			asking.set(id, resolve);
			onProgress({
				type: "host_key",
				setup: id,
				host,
				key_type: "ED25519",
				fingerprint: FINGERPRINT,
			});
		});
		asking.delete(id);
		if (!ok) return stop("host_key", "You didn't trust the host key, so nothing ran");
		await step("install", "Checking the system and copying maxd");
		if (host === "build-box") {
			const text = "maxd can't keep running after you log out until lingering is on.";
			onProgress({
				type: "hint",
				setup: id,
				step: "install",
				text,
				command: "loginctl enable-linger $USER",
			});
			return failed(`${text} Run the command on ${host}, then try again`);
		}
		await step("key", "Adding MaxHQ's key");
		await step("handshake", "Connecting through the new key");
		await step("secrets", "Sending the logins you picked");
		const device = `dev-${host}`;
		const link = `link-${host}`;
		const at = now();
		change((base) => [
			...base,
			{
				entity: "device",
				id: device,
				name: host,
				os: "linux",
				boot_id: `${host}-boot-1`,
				maxd_version: "0.1.0",
				winding_down_until: null,
				workspace_host: null,
				build: null,
				target: "x86_64-unknown-linux-musl",
			},
			{
				entity: "device_link",
				id: link,
				name: host,
				device,
				route: { type: "ssh", host },
				state: { state: "connected", since: at },
				network: null,
				peer: null,
				via: [],
				capabilities: {
					transfers: true,
					tunnels: true,
					workspaces: false,
					inbox: true,
					load: true,
					updates: true,
					tools: true,
				},
				waiting: [],
				routes: [{ route: { type: "ssh", host }, last_good_here: true }],
			},
		]);
		if (found) found.link = link;
		else hosts.push({ host, from: "ssh_config", address: null, link });
		onProgress({ type: "done", setup: id, link, device });
		return { status: "ok", data: link };
	};

	return {
		readLoad: async (devices) => {
			const at = now();
			return devices.map((device) => {
				const sample = LOADS[device];
				return {
					device,
					load: sample && connected(device) ? { ...sample, sampled_at: at } : null,
					received_at: at,
				};
			});
		},
		readDiagnostics: async (device, windowSecs, bucketSecs) => {
			if (!known(device)) {
				return { status: "error", error: { kind: "not_found", what: `device ${device}` } };
			}
			if (!connected(device))
				return { status: "error", error: { kind: "not_connected", device } };
			return { status: "ok", data: diagnostics(device, windowSecs, bucketSecs) };
		},
		updateTools: async (device, tools) => {
			if (!known(device)) {
				return { status: "error", error: { kind: "not_found", what: `device ${device}` } };
			}
			if (!connected(device))
				return { status: "error", error: { kind: "not_connected", device } };
			const setTools = (edit: (tool: ToolInfo) => ToolInfo) =>
				change((base) =>
					base.map((e) =>
						e.entity === "device_tools" && e.device === device
							? {
									...e,
									tools: e.tools.map((t) =>
										tools.some((x) => samePlace(t, x)) ? edit(t) : t
									),
								}
							: e
					)
				);
			setTools((t) => ({ ...t, state: { type: "running", message: "Downloading" } }));
			void pause(stepMs * 2).then(() =>
				setTools((t) => ({ ...t, version: t.latest ?? t.version, state: { type: "idle" } }))
			);
			return { status: "ok", data: { status: "done" } };
		},
		updateFleet: async (devices, tools, onProgress) => {
			for (const device of devices) onProgress({ device, status: { status: "waiting" } });
			await Promise.all(devices.map((device) => fleetStep(device, tools, onProgress)));
			return OK;
		},
		setupDevice: (host, _secrets, onProgress) => setup(host, onProgress),
		confirmHostKey: async (id, ok) => {
			const resolve = asking.get(id);
			if (!resolve)
				return { status: "error", error: { kind: "not_found", what: `setup ${id}` } };
			resolve(ok);
			return OK;
		},
		setRoutes: async (link, hosts) => {
			const found = all().find((e) => e.entity === "device_link" && e.id === link);
			if (found?.entity !== "device_link") {
				return { status: "error", error: { kind: "not_found", what: `device ${link}` } };
			}
			const cleaned = hosts.map((h) => h.trim()).filter((h) => h !== "");
			if (cleaned.length === 0) return failed("Give the device at least one route");
			const bad = cleaned.find((h) => /\s/.test(h) || h.startsWith("-"));
			if (bad) return failed(`"${bad}" isn't an SSH host`);
			const good = found.routes?.find((r) => r.last_good_here)?.route;
			change((base) =>
				base.map((e) =>
					e.entity === "device_link" && e.id === link
						? {
								...e,
								route: { type: "ssh", host: cleaned[0]! },
								routes: cleaned.map((host) => ({
									route: { type: "ssh", host },
									last_good_here: good?.type === "ssh" && good.host === host,
								})),
							}
						: e
				)
			);
			return OK;
		},
		listSetupHosts: async () => hosts.map((h) => ({ ...h })),
	};
};
