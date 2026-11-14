// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, PermissionMode } from "../bindings";
import { deviceDefault } from "../settings/permission";
import { account, agent, device, entities, link, NOW, project, workspace } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import type { DeviceLoad } from "../data/source";
import {
	accountOptions,
	balanceCandidates,
	balanceNote,
	type Choices,
	emptyChoices,
	modelOptions,
	modeOptions,
	pickBalanced,
	pickChoices,
	planStart,
	resolve,
	startFailure,
	workspaceOptions,
} from "./spec";

const MODELS = [
	{ id: "opus", label: "Opus", default: true },
	{ id: "sonnet", label: "Sonnet", default: false },
];

const settings = (device: string, mode: PermissionMode): Entity => ({
	entity: "device_settings",
	device,
	default_permission: mode,
	workspace_network: null,
	workspace_pool: null,
});

const viewOf = (extra: Entity[] = []): ViewState =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [
					device({ id: "dev-mac", name: "studio", os: "macos" }),
					device({ id: "dev-node" }),
					device({ id: "dev-off" }),
				],
				links: [
					link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
					link({ id: "node", name: "devbox", device: "dev-node" }),
					link({
						id: "off",
						name: "laptop",
						device: "dev-off",
						state: {
							state: "unreachable",
							reason: "Tailscale is off",
							last_seen: null,
						},
					}),
				],
				accounts: [
					account({ id: "a-claude", models: MODELS }),
					account({ id: "a-codex", vendor: "codex", label: "Codex" }),
					account({ id: "a-node", device: "dev-node" }),
				],
				projects: [project("p1", "MaxHQ"), project("p2", "Comet")],
				workspaces: [
					workspace({ id: "ws-mac", project: "p1" }),
					workspace({
						id: "ws-node",
						project: "p1",
						device: "dev-node",
						location: { type: "container", instance: "maxhq", path: "/work/maxhq" },
					}),
					workspace({ id: "ws-off", project: "p2", device: "dev-off" }),
				],
			}),
			...extra,
		],
	});

const choose = (over: Partial<Choices>): Choices => ({ ...emptyChoices("p1"), ...over });

const specOf = (view: ViewState, choices: Choices) => {
	const plan = planStart(view, choices);
	if (!plan.ok) throw new Error(plan.reason);
	return plan.spec;
};

describe("the spec", () => {
	it("sends each choice for a project's checkout", () => {
		const spec = specOf(
			viewOf(),
			choose({
				workspace: "ws-mac",
				account: "a-claude",
				model: "sonnet",
				mode: "accept_edits",
				title: "  Fix the build ",
				first: "Make the tests pass",
			})
		);
		expect(spec).toEqual({
			vendor: "claude",
			account: "a-claude",
			place: { type: "workspace", workspace: "ws-mac" },
			model: "sonnet",
			permission: "accept_edits",
			title: "Fix the build",
			first: { text: "Make the tests pass", attachments: [] },
			branch: null,
		});
	});

	it("leaves the model, title and first message to the CLI when you don't pick them", () => {
		const spec = specOf(viewOf(), choose({ workspace: "ws-mac" }));
		expect(spec.model).toBeNull();
		expect(spec.title).toBeNull();
		expect(spec.first).toBeNull();
	});

	it("follows a checkout on another device to that device's logins", () => {
		const spec = specOf(viewOf(), choose({ workspace: "ws-node", account: "a-claude" }));
		expect(spec.place).toEqual({ type: "workspace", workspace: "ws-node" });
		expect(spec.account).toBe("a-node");
	});

	it("sends a folder on the device you picked", () => {
		const spec = specOf(
			viewOf(),
			choose({ place: "folder", device: "dev-node", path: " /srv/scratch " })
		);
		expect(spec.place).toEqual({ type: "folder", path: "/srv/scratch" });
		expect(spec.account).toBe("a-node");
	});

	it("puts a folder on this Mac unless you pick a device", () => {
		const r = resolve(viewOf(), choose({ place: "folder" }));
		expect(r.device).toBe("dev-mac");
		expect(r.workspace).toBeUndefined();
	});

	it("says why it can't start yet", () => {
		const view = viewOf();
		const reason = (choices: Choices) => {
			const plan = planStart(view, choices);
			return plan.ok ? null : plan.reason;
		};
		expect(reason(choose({ place: "folder", path: "src/app" }))).toBe(
			"Type the folder's full path, starting with /"
		);
		expect(reason(choose({ place: "folder", device: "dev-off", path: "/work" }))).toBe(
			"laptop isn't connected"
		);
		expect(reason(choose({ project: "p2" }))).toBe("laptop isn't connected");
	});

	it("turns off the checkouts on devices that aren't connected", () => {
		expect(workspaceOptions(viewOf(), "p2")).toEqual([
			expect.objectContaining({ value: "ws-off", disabled: true, detail: "Not connected" }),
		]);
	});

	it("offers only the logins on the device", () => {
		const options = accountOptions(viewOf(), "dev-mac", NOW).map((o) => o.value);
		expect(options).toEqual(["a-claude", "a-codex"]);
	});

	it("shows each login's own windows, by length, and none it doesn't report", () => {
		const usage = (windows: [number, number][]) => ({
			checked_at: NOW,
			source: "claude_cache" as const,
			windows: windows.map(([window_minutes, used_percent]) => ({
				scope: null,
				window_minutes,
				used_percent,
				resets_at: null,
			})),
		});
		const view = viewOf([
			{
				entity: "account",
				...account({
					id: "a-claude",
					usage: usage([
						[10080, 38],
						[300, 62],
					]),
				}),
			},
			{
				entity: "account",
				...account({ id: "a-codex", vendor: "codex", usage: usage([[10080, 64]]) }),
			},
		]);
		const bars = accountOptions(view, "dev-mac", NOW).map((o) =>
			o.usage?.map((w) => [w.minutes, w.percent])
		);
		expect(bars).toEqual([
			[
				[300, 62],
				[10080, 38],
			],
			[[10080, 64]],
		]);
	});

	it("offers the CLI default when no login of the vendor lists models", () => {
		const view = viewOf();
		expect(modelOptions(view, view.tables.account.get("a-codex"))).toEqual([
			expect.objectContaining({ value: "", label: "CLI default" }),
		]);
		const r = resolve(view, choose({ workspace: "ws-mac", account: "a-claude" }));
		expect(r.shownModel).toBe("opus");
		expect(r.model).toBeNull();
	});

	it("borrows the vendor's models for a login that lists none yet", () => {
		const view = viewOf();
		expect(modelOptions(view, view.tables.account.get("a-node")).map((o) => o.value)).toEqual([
			"opus",
			"sonnet",
		]);
		const r = resolve(
			view,
			choose({ workspace: "ws-node", account: "a-node", model: "sonnet" })
		);
		expect(r.model).toBe("sonnet");
		expect(specOf(view, choose({ workspace: "ws-node", model: "sonnet" })).model).toBe(
			"sonnet"
		);
	});
});

describe("the mode", () => {
	it("starts in the device's saved default", () => {
		const view = viewOf([settings("dev-mac", "ask"), settings("dev-node", "accept_edits")]);
		expect(resolve(view, choose({ workspace: "ws-mac" })).mode).toBe("ask");
		expect(specOf(view, choose({ workspace: "ws-node" })).permission).toBe("accept_edits");
	});

	it("falls back to Auto on a Mac and Full access elsewhere", () => {
		const view = viewOf();
		expect(deviceDefault(view, "dev-mac")).toBe("auto");
		expect(deviceDefault(view, "dev-node")).toBe("full_access");
		expect(resolve(view, choose({ workspace: "ws-mac" })).mode).toBe("auto");
		expect(resolve(view, choose({ workspace: "ws-node" })).mode).toBe("full_access");
	});

	it("keeps the mode you picked over the default", () => {
		const spec = specOf(viewOf(), choose({ workspace: "ws-mac", mode: "plan" }));
		expect(spec.permission).toBe("plan");
	});

	it("turns Plan off for Codex, and says so before a login is picked", () => {
		const plan = (vendor?: "claude" | "codex" | "cursor") =>
			modeOptions(vendor).find((o) => o.value === "plan");
		expect(plan("codex")).toMatchObject({ disabled: true, tag: "Not in Codex" });
		expect(plan("claude")).toMatchObject({ disabled: false, tag: undefined });
		expect(plan("cursor")).toMatchObject({ disabled: false, tag: undefined });
		expect(plan()).toMatchObject({ disabled: false, tag: "Not in Codex" });
	});

	it("drops Plan for Codex, to the device's default or else to asking first", () => {
		const picked = specOf(
			viewOf(),
			choose({ workspace: "ws-mac", account: "a-codex", mode: "plan" })
		);
		expect(picked.permission).toBe("auto");
		const byDefault = specOf(
			viewOf([settings("dev-mac", "plan")]),
			choose({ workspace: "ws-mac", account: "a-codex" })
		);
		expect(byDefault.permission).toBe("ask");
	});
});

describe("the branch", () => {
	it("sends the branch you typed for a container", () => {
		const spec = specOf(viewOf(), choose({ workspace: "ws-node", branch: " fix/login " }));
		expect(spec.branch).toBe("fix/login");
	});

	it("leaves the name to maxd when you type none", () => {
		expect(specOf(viewOf(), choose({ workspace: "ws-node" })).branch).toBeNull();
	});

	it("sends none outside a container", () => {
		expect(specOf(viewOf(), choose({ workspace: "ws-mac", branch: "fix" })).branch).toBeNull();
	});

	it("won't start on a name the guest would refuse, and says why", () => {
		expect(planStart(viewOf(), choose({ workspace: "ws-node", branch: "fix login" }))).toEqual({
			ok: false,
			reason: "A branch can't have spaces.",
		});
		const taken = viewOf(
			entities({ agents: [agent({ id: "a1", workspace: "ws-node", branch: "fix/login" })] })
		);
		expect(planStart(taken, choose({ workspace: "ws-node", branch: "fix-login" }))).toEqual({
			ok: false,
			reason: "fix/login already has this branch's folder. Pick another name.",
		});
	});

	it("drops the branch when the place moves out of a container", () => {
		const view = viewOf();
		const typed = pickChoices(view, choose({ workspace: "ws-node" }), { branch: "fix" });
		expect(typed.branch).toBe("fix");
		expect(pickChoices(view, typed, { title: "Fix it" }).branch).toBe("fix");
		expect(pickChoices(view, typed, { workspace: "ws-mac" }).branch).toBe("");
		expect(pickChoices(view, typed, { place: "folder" }).branch).toBe("");
	});
});

describe("a failed start", () => {
	it("says what went wrong", () => {
		expect(
			startFailure({ status: "error", error: { kind: "not_connected", device: "devbox" } })
		).toBe("devbox isn't connected");
		expect(
			startFailure({
				status: "ok",
				data: { status: "refused", reason: "Codex is at its limit" },
			})
		).toBe("Codex is at its limit");
	});
});

describe("the balancer's pick", () => {
	const GIB = 1024 ** 3;
	const load = (device: string, cpu: number): DeviceLoad => ({
		device,
		load: {
			sampled_at: NOW,
			cpu_util: cpu,
			cpu_count: 8,
			mem_available: 8 * GIB,
			mem_total: 16 * GIB,
		},
		received_at: NOW,
	});
	const busyMac = [load("dev-mac", 0.9), load("dev-node", 0.2)];

	it("weighs the connected devices that hold the project with a login", () => {
		expect(balanceCandidates(viewOf(), choose({}))).toEqual(["dev-mac", "dev-node"]);
		expect(balanceCandidates(viewOf(), choose({ place: "folder" }))).toEqual([
			"dev-mac",
			"dev-node",
		]);
	});

	it("puts the agent on the freer device and says why", () => {
		const view = viewOf();
		const winner = pickBalanced(view, balanceCandidates(view, choose({})), busyMac, NOW);
		expect(winner).toBe("dev-node");
		const r = resolve(view, choose({}), winner);
		expect(r.workspace?.id).toBe("ws-node");
		expect(r.balanced).toBe(true);
		expect(balanceNote(view, "dev-node")).toBe("devbox: most free CPU and memory");
	});

	it("leaves it to you once you pick a place", () => {
		const view = viewOf();
		expect(balanceCandidates(view, choose({ workspace: "ws-mac" }))).toEqual([]);
		const r = resolve(view, choose({ workspace: "ws-mac" }), "dev-node");
		expect(r.workspace?.id).toBe("ws-mac");
		expect(r.balanced).toBe(false);
		expect(balanceCandidates(view, choose({ place: "folder", device: "dev-mac" }))).toEqual([]);
	});

	it("keeps only devices with a login for the vendor you picked", () => {
		expect(
			balanceCandidates(viewOf(), choose({ place: "folder", account: "a-codex" }))
		).toEqual([]);
	});

	it("leans toward a device you prefer", () => {
		const view = viewOf([
			{
				entity: "device_settings",
				device: "dev-mac",
				default_permission: "auto",
				workspace_network: null,
				workspace_pool: null,
				placement: "prefer",
			} as Entity,
		]);
		const even = [load("dev-mac", 0.5), load("dev-node", 0.3)];
		expect(pickBalanced(view, ["dev-mac", "dev-node"], even, NOW)).toBe("dev-mac");
	});

	it("falls back to the usual pick without fresh samples", () => {
		const view = viewOf();
		expect(pickBalanced(view, ["dev-mac", "dev-node"], [], NOW)).toBeNull();
		expect(resolve(view, choose({}), null).workspace?.id).toBe("ws-mac");
	});
});

describe("a local ACP agent in the form", () => {
	const gemini = (plan: boolean, models = MODELS.slice(0, 0)) =>
		account({
			id: "a-gemini",
			vendor: "acp",
			label: "Gemini",
			login: "acp_command",
			config_dir: "",
			email: null,
			models,
			acp: { images: false, load: false, plan },
		});
	const definition: Entity = {
		entity: "acp_agent",
		id: "acp-1",
		device: "dev-mac",
		name: "Gemini",
		program: "gemini",
		args: ["--experimental-acp"],
	};
	const withGemini = (plan = false) =>
		viewOf([{ entity: "account", ...gemini(plan) }, definition]);

	it("shows beside the others, called by its name, with its program", () => {
		const option = accountOptions(withGemini(), "dev-mac", NOW).find(
			(o) => o.value === "a-gemini"
		);
		expect(option).toMatchObject({ label: "Gemini", detail: "ACP agent, gemini" });
	});

	it("offers the agent's default model when it lists none", () => {
		expect(modelOptions(withGemini(), gemini(false))).toEqual([
			{ value: "", label: "Agent default", detail: "The agent didn't list its models" },
		]);
	});

	it("hides Auto, and shows Plan only when the agent has a plan mode", () => {
		const modes = (plan: boolean) =>
			modeOptions("acp", { plan }).map((o) => [o.value, o.disabled, o.tag]);
		expect(modes(false)).toEqual([
			["ask", false, undefined],
			["accept_edits", false, undefined],
			["full_access", false, undefined],
		]);
		expect(modes(true)).toContainEqual(["plan", false, undefined]);
		expect(modes(true).map(([mode]) => mode)).not.toContain("auto");
	});

	it("starts in Accept edits on a device that defaults to Auto", () => {
		const spec = specOf(withGemini(), choose({ workspace: "ws-mac", account: "a-gemini" }));
		expect(spec.permission).toBe("accept_edits");
	});

	it("drops Plan for an agent without one, and keeps it for one with it", () => {
		const picked = (plan: boolean) =>
			specOf(
				withGemini(plan),
				choose({ workspace: "ws-mac", account: "a-gemini", mode: "plan" })
			).permission;
		expect(picked(false)).toBe("accept_edits");
		expect(picked(true)).toBe("plan");
	});
});
