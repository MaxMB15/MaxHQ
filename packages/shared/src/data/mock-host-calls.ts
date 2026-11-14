// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The host, tool, snapshot and move calls of the sample data. Each changes
// the sample entities the way `maxd` would publish the change: a check
// stamps the report, an apply fixes what it can, an install runs through
// its phases, a move step runs and lands on the next, and an install of
// the Android SDK waits on its licence. A device whose link doesn't offer
// the call is refused the way the app refuses it.

import type {
	Capabilities,
	DeviceId,
	Entity,
	Move,
	MoveStep,
	ToolInfo,
	ToolKind,
	ToolPlace,
	ToolState,
	WorkspaceSnapshot,
} from "../bindings";
import { sameKind, samePlace } from "../hosts/tools";
import { MOVE_STEPS } from "../hosts/moves";
import { rollbackLoses } from "../hosts/unsaved";
import type { FleetContext } from "./mock-fleet";
import type { ActionResult, DataSource } from "./source";

/** The calls this module answers. */
export type HostCalls = Pick<
	DataSource,
	| "checkHost"
	| "applyHost"
	| "installTool"
	| "removeTool"
	| "listSnapshots"
	| "restoreSnapshot"
	| "planMove"
	| "runMoveStep"
	| "moveBack"
	| "retireMove"
	| "installAcpAgent"
	| "acceptLicence"
>;

const DAY = 24 * 60 * 60 * 1000;

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

const gone = (what: string): ActionResult => ({
	status: "error",
	error: { kind: "not_found", what },
});

const pause = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

const notYet = async (): Promise<ActionResult> => refused("This device doesn't offer it yet.");

/** The host calls with nothing behind them, refused the way an older `maxd` does. */
export const noHosts: HostCalls = {
	checkHost: notYet,
	applyHost: notYet,
	installTool: notYet,
	removeTool: notYet,
	listSnapshots: notYet,
	restoreSnapshot: notYet,
	planMove: notYet,
	runMoveStep: notYet,
	moveBack: notYet,
	retireMove: notYet,
	installAcpAgent: notYet,
	acceptLicence: notYet,
};

type Flag = "host_spec" | "snapshots" | "moves" | "tool_installs";

const TOO_OLD: Readonly<Record<Flag, string>> = {
	host_spec: "check its host",
	snapshots: "keep snapshots",
	moves: "move workspaces",
	tool_installs: "install tools",
};

/** The host calls over the sample entities. */
export const mockHostCalls = (context: FleetContext): HostCalls => {
	const { now, change, stepMs } = context;
	const all = () => [...context.entities()];

	/** Why `device` can't take a call that needs `flag`, or null when it can. */
	const lacks = (device: DeviceId, flag: Flag): ActionResult | null => {
		const link = all().find((e) => e.entity === "device_link" && e.device === device);
		if (link?.entity !== "device_link") return gone("This device");
		const does = link.capabilities as Capabilities | null;
		if (does && does[flag] !== true) {
			return {
				status: "error",
				error: {
					kind: "failed",
					message: `${link.name}'s maxd is too old to ${TOO_OLD[flag]}. Update it first`,
				},
			};
		}
		return null;
	};

	const editTools = (device: DeviceId, edit: (tools: ToolInfo[]) => ToolInfo[]) =>
		change((base) =>
			base.map((e) =>
				e.entity === "device_tools" && e.device === device
					? { ...e, tools: edit(e.tools) }
					: e
			)
		);

	const toolOf = (device: DeviceId, place: ToolPlace, kind: ToolKind) => {
		const tools = all().find((e) => e.entity === "device_tools" && e.device === device);
		return tools?.entity === "device_tools"
			? tools.tools.find((t) => samePlace(t.place, place) && sameKind(t.kind, kind))
			: undefined;
	};

	const setTool = (
		device: DeviceId,
		place: ToolPlace,
		kind: ToolKind,
		patch: Partial<ToolInfo>
	) =>
		editTools(device, (tools) => {
			const at = tools.findIndex((t) => samePlace(t.place, place) && sameKind(t.kind, kind));
			if (at >= 0) return tools.map((t, i) => (i === at ? { ...t, ...patch } : t));
			const added: ToolInfo = {
				kind,
				place,
				path: null,
				version: null,
				latest: null,
				owner: "max_hq",
				method: "maxhq",
				update: { type: "auto" },
				state: { type: "idle" },
				...patch,
			};
			return [...tools, added];
		});

	/** Runs an install through its phases, ending at `version`. */
	const runInstall = async (
		device: DeviceId,
		place: ToolPlace,
		kind: ToolKind,
		version: string
	) => {
		const total = 80_000_000;
		const running = (state: Partial<Extract<ToolState, { type: "running" }>>): ToolState => ({
			type: "running",
			message: "Installing",
			...state,
		});
		const steps: ToolState[] = [
			running({ phase: "resolving" }),
			running({ phase: "downloading", bytes_done: 0, bytes_total: total }),
			running({ phase: "downloading", bytes_done: total / 2, bytes_total: total }),
			running({ phase: "verifying" }),
			running({ phase: "installing" }),
			running({ phase: "linking" }),
		];
		for (const state of steps) {
			setTool(device, place, kind, { state });
			await pause(stepMs);
		}
		setTool(device, place, kind, { version, latest: version, state: { type: "idle" } });
	};

	const editMove = (id: string, edit: (move: Move) => Move) =>
		change((base) =>
			base.map((e) => (e.entity === "move" && e.id === id ? { ...e, ...edit(e) } : e))
		);

	const moveOf = (id: string) => {
		const found = all().find((e) => e.entity === "move" && e.id === id);
		return found?.entity === "move" ? found : undefined;
	};

	const snapshots = (): WorkspaceSnapshot[] => [
		{ name: "snap-daily-1", taken_at: now() - 2 * DAY, expires_at: now() + 5 * DAY },
		{ name: "snap-daily-2", taken_at: now() - DAY, expires_at: now() + 6 * DAY },
		{ name: "snap-before-move", taken_at: now() - 3 * 60 * 60 * 1000, expires_at: null },
	];

	const containerOf = (workspace: string) => {
		const found = all().find((e) => e.entity === "workspace" && e.id === workspace);
		return found?.entity === "workspace" ? found : undefined;
	};

	return {
		checkHost: async (device) => {
			const no = lacks(device, "host_spec");
			if (no) return no;
			change((base) =>
				base.map((e) =>
					e.entity === "host_report" && e.device === device
						? { ...e, checked_at: now() }
						: e
				)
			);
			return DONE;
		},
		applyHost: async (device, items) => {
			const no = lacks(device, "host_spec");
			if (no) return no;
			change((base) =>
				base.map((e) =>
					e.entity === "host_report" && e.device === device
						? {
								...e,
								checked_at: now(),
								items: e.items.map((item) =>
									items.includes(item.id) && item.fix?.type === "now"
										? { ...item, state: { state: "ok" }, fix: null }
										: item
								),
							}
						: e
				)
			);
			return DONE;
		},
		installTool: async (device, place, tool, version) => {
			const no = lacks(device, "tool_installs");
			if (no) return no;
			if (toolOf(device, place, tool)?.state.type === "running") {
				return refused("It's installing already");
			}
			if (tool.type === "helper" && tool.tool === "android_sdk") {
				setTool(device, place, tool, {
					state: {
						type: "licences",
						pending: [
							{
								id: "android-sdk-license",
								text: "Terms and conditions for the Android Software Development Kit.",
								text_sha256:
									"0d6f4b8e2c1a9f7d5b3e1c0a8f6d4b2e0c9a7f5d3b1e9c7a5f3d1b0e8c6a4f2d",
							},
						],
					},
				});
				return DONE;
			}
			void runInstall(device, place, tool, version ?? "1.0.0");
			return DONE;
		},
		removeTool: async (device, place, tool) => {
			const no = lacks(device, "tool_installs");
			if (no) return no;
			if (!toolOf(device, place, tool)) return gone("This tool");
			editTools(device, (tools) =>
				tools.filter((t) => !(samePlace(t.place, place) && sameKind(t.kind, tool)))
			);
			return DONE;
		},
		listSnapshots: async (workspace) => {
			const found = containerOf(workspace);
			if (!found) return gone("This workspace");
			const no = lacks(found.device, "snapshots");
			if (no) return no;
			if (found.location.type !== "container") {
				return refused("Only container workspaces have snapshots");
			}
			return { status: "ok", data: { status: "snapshots", snapshots: snapshots() } };
		},
		restoreSnapshot: async (workspace, snapshot, asNew, discardUnsaved) => {
			const found = containerOf(workspace);
			if (!found) return gone("This workspace");
			const no = lacks(found.device, "snapshots");
			if (no) return no;
			if (!snapshots().some((s) => s.name === snapshot)) return gone("This snapshot");
			const loses = rollbackLoses(found, asNew);
			if (loses && !discardUnsaved) {
				return refused(
					`Rolling back loses this: ${loses}. Push it first, or restore as new`
				);
			}
			await pause(stepMs);
			return DONE;
		},
		planMove: async (device, instance) => {
			const no = lacks(device, "moves");
			if (no) return no;
			if (
				all().some(
					(e) => e.entity === "move" && e.device === device && e.source === instance
				)
			) {
				return refused(`${instance} is moving already`);
			}
			const name = instance.replace(/^ws-/, "");
			const added: Entity = {
				entity: "move",
				id: `move-${name}-${now()}`,
				device,
				source: instance,
				target: `mhq-${name}`,
				workspace: null,
				step: "look",
				state: { state: "ready" },
				plan: null,
				source_changed: false,
				kept_until: null,
			};
			change((base) => [...base, added]);
			return DONE;
		},
		runMoveStep: async (moving, step) => {
			const found = moveOf(moving);
			if (!found) return gone("This move");
			const no = lacks(found.device, "moves");
			if (no) return no;
			if (found.state.state === "running") return refused("A step is running already");
			const copyAgain = step === "copy" && found.step === "switch";
			if (step !== found.step && !copyAgain) return refused("That step isn't next");
			editMove(moving, (m) => ({
				...m,
				state: { state: "running", doing: `Running ${step}` },
			}));
			void pause(stepMs * 2).then(() =>
				editMove(moving, (m): Move => {
					if (copyAgain)
						return { ...m, source_changed: false, state: { state: "ready" } };
					const next: MoveStep = MOVE_STEPS[MOVE_STEPS.indexOf(step) + 1] ?? "retire";
					return {
						...m,
						step: next,
						state: { state: "ready" },
						kept_until: next === "keep" ? now() + 7 * DAY : m.kept_until,
					};
				})
			);
			return DONE;
		},
		moveBack: async (moving) => {
			const found = moveOf(moving);
			if (!found) return gone("This move");
			const no = lacks(found.device, "moves");
			if (no) return no;
			if (found.step !== "keep")
				return refused("A move goes back only while the original is kept");
			editMove(moving, (m) => ({ ...m, state: { state: "moved_back" } }));
			return DONE;
		},
		retireMove: async (moving) => {
			const found = moveOf(moving);
			if (!found) return gone("This move");
			const no = lacks(found.device, "moves");
			if (no) return no;
			if (found.step !== "keep")
				return refused("The original is retired only after the switch");
			editMove(moving, (m) => ({ ...m, step: "retire", state: { state: "retired" } }));
			return DONE;
		},
		installAcpAgent: async (device, agent, version, place, confirmUnchecked) => {
			const no = lacks(device, "tool_installs");
			if (no) return no;
			const registry = all().find((e) => e.entity === "acp_registry" && e.device === device);
			const listed =
				registry?.entity === "acp_registry"
					? registry.agents.find((a) => a.id === agent && a.version === version)
					: undefined;
			if (!listed) return gone("This agent");
			if (!listed.install) return refused(`${listed.name} has no build for this device`);
			if (listed.install.type === "binary" && !listed.install.checked && !confirmUnchecked) {
				return refused(
					`The registry gives no checksum for ${listed.name}. Confirm to install it`
				);
			}
			void runInstall(device, place, { type: "acp", id: agent }, version).then(() =>
				change((base) =>
					base.map((e) =>
						e.entity === "acp_registry" && e.device === device
							? {
									...e,
									agents: e.agents.map((a) =>
										a.id === agent ? { ...a, installed: version } : a
									),
								}
							: e
					)
				)
			);
			return DONE;
		},
		acceptLicence: async (device, licence, textSha256) => {
			const no = lacks(device, "tool_installs");
			if (no) return no;
			const tools = all().find((e) => e.entity === "device_tools" && e.device === device);
			const waiting =
				tools?.entity === "device_tools"
					? tools.tools.find(
							(t) =>
								t.state.type === "licences" &&
								t.state.pending.some((p) => p.id === licence)
						)
					: undefined;
			if (!waiting || waiting.state.type !== "licences") return gone("This licence");
			const pending = waiting.state.pending.find((p) => p.id === licence);
			if (pending?.text_sha256 !== textSha256) {
				return refused("The licence changed since you read it");
			}
			const left = waiting.state.pending.filter((p) => p.id !== licence);
			if (left.length > 0) {
				setTool(device, waiting.place, waiting.kind, {
					state: { type: "licences", pending: left },
				});
			} else {
				void runInstall(device, waiting.place, waiting.kind, "36.0.0");
			}
			return DONE;
		},
	};
};
