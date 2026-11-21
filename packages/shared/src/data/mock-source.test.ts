// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";
import type { Activity, Delta, TimelineUpdate } from "../bindings";

import { isLocalAccount } from "../status/agent-kind";
import { agentConnection, type ConnectionLook } from "../status/connection";
import { all, applyDelta, applySnapshot, idOf, type ViewState } from "../view/replica";
import { DEFAULT_INBOX, inboxOf } from "../view/inbox";
import { agentLinkContext, deviceGroups } from "../view/select";
import { NOW } from "../test/build";
import { mockSource } from "./mock-source";
import {
	CLAUDE,
	CODEX,
	CURSOR,
	DEVBOX_LINK,
	DEVBOX,
	GEMINI,
	GEMINI_ACCOUNT,
	MAC,
	SCENARIOS,
} from "./mock-view";

const source = (scenario?: string) => mockSource({ now: () => NOW, tick: false, scenario });

const load = async (scenario?: string) => applySnapshot(await source(scenario).snapshot());

const ACTIVITY_STATES: Activity["state"][] = [
	"starting",
	"busy",
	"busy_in_subagents",
	"needs_input",
	"done_unread",
	"idle_cache_warm",
	"idle",
	"waiting_for_limit_reset",
	"paused",
	"error",
	"not_accessible",
];

const looksIn = (view: ViewState): Set<ConnectionLook> => {
	const looks = new Set<ConnectionLook>();
	for (const group of deviceGroups(view, NOW)) {
		for (const device of group.devices) looks.add(device.look);
	}
	for (const agent of all(view, "agent")) {
		looks.add(agentConnection(agent, agentLinkContext(view, agent), NOW).look);
	}
	return looks;
};

describe("mock source", () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	it("shows every activity state in the default setup", async () => {
		const view = await load();
		const states = new Set(all(view, "agent").map((a) => a.activity.state));
		expect([...states].sort()).toEqual([...ACTIVITY_STATES].sort());
	});

	it("shows every connection look across its setups", async () => {
		const looks = new Set<ConnectionLook>();
		for (const scenario of SCENARIOS) {
			for (const look of looksIn(await load(scenario.id))) looks.add(look);
		}
		expect([...looks].sort()).toEqual(
			["agent_down", "asleep", "connecting", "live", "unknown", "unreachable"].sort()
		);
	});

	it("has Claude Max, Codex Pro and Cursor Pro logins, metered except Cursor", async () => {
		const logins = all(await load(), "account").filter(
			(a) => a.origin === null && a.vendor !== "acp" && !isLocalAccount(a)
		);
		expect(logins.map((a) => [a.vendor, a.plan])).toEqual([
			["claude", "max"],
			["codex", "pro"],
			["cursor", "pro"],
		]);
		for (const account of logins) {
			const windows = account.vendor === "cursor" ? undefined : [300, 10080];
			const own = account.usage?.windows.filter((w) => w.scope === null);
			expect(own?.map((w) => w.window_minutes)).toEqual(windows);
			// Each metered plan also has limits for single models.
			const scoped = account.usage?.windows.some((w) => w.scope !== null);
			expect(scoped).toBe(account.vendor === "cursor" ? undefined : true);
			expect(account.email).toMatch(/@example\.(com|net)$/);
		}
	});

	it("has three local logins, none metered, two of them on the Mac's server", async () => {
		const local = all(await load(), "account").filter(isLocalAccount);
		expect(local.map((a) => [a.device, a.model_server]).sort()).toEqual([
			[DEVBOX, "srv-devbox-ollama"],
			[DEVBOX, "srv-mac-llama"],
			[MAC, "srv-mac-llama"],
		]);
		for (const account of local) {
			expect(account.vendor).toBe("claude");
			expect(account.usage).toBeNull();
			expect(account.plan).toBeNull();
		}
	});

	it("shows Cursor's month only while the Mac's Cursor usage switch is on", async () => {
		const mock = source();
		const turn = async (cursor_usage: boolean) => {
			await mock.setDeviceSettings({
				device: "dev-mac",
				default_permission: "auto",
				workspace_network: null,
				workspace_pool: null,
				auto_settle_days: 5,
				snooze_at_limit: false,
				continue_claude_at_limit: false,
				continue_codex_at_limit: false,
				cursor_usage,
			});
			return applySnapshot(await mock.snapshot()).tables.account.get(CURSOR)!.usage;
		};
		const on = await turn(true);
		expect(on?.windows.map((w) => [w.scope, w.period, w.used_percent])).toEqual([
			[null, "billing_cycle", 38],
			["auto", "billing_cycle", 24],
			["api", "billing_cycle", 61],
			["on_demand", "billing_cycle", 24.8],
		]);
		expect(on?.windows[3]!.spend).toEqual({ used_cents: 1240, limit_cents: 5000 });
		expect(on?.windows[0]!.resets_at).toBeGreaterThan(NOW);
		expect(await turn(false)).toBeNull();
	});

	it("links devbox's accounts to those logins without making up usage", async () => {
		const linked = all(await load(), "account").filter((a) => a.origin !== null);
		expect(linked.map((a) => [a.device, a.vendor, a.login, a.origin])).toEqual([
			[DEVBOX, "claude", "setup_token", CLAUDE],
			[DEVBOX, "codex", "chat_gpt_sign_in", CODEX],
		]);
		for (const account of linked) {
			expect(account.usage).toBeNull();
			expect(account.plan).toBeNull();
		}
	});

	it("runs devbox's agents in its containers beside the one MaxHQ made", async () => {
		const view = await load();
		const nl = deviceGroups(view, NOW)
			.flatMap((g) => g.devices)
			.find((d) => d.link.id === DEVBOX_LINK);
		expect(nl?.workspaces.map((w) => w.name)).toEqual([
			"mhq-maxhq",
			"ws-alpha",
			"ws-bravo",
			"ws-charlie",
			"ws-echo",
			"ws-example-app",
			"ws-example-app-cursor",
		]);
	});

	it("makes a workspace and walks it to running", async () => {
		vi.useFakeTimers();
		const mock = source();
		const deltas: Delta[] = [];
		await mock.subscribe((d) => deltas.push(d));
		const made = await mock.createWorkspace({
			project: "proj-comet",
			device: DEVBOX,
			name: null,
			start_at_boot: false,
		});
		expect(made).toMatchObject({ status: "ok", data: { status: "created" } });
		await vi.runAllTimersAsync();
		const view = applySnapshot(await mock.snapshot());
		const workspace = all(view, "workspace").find(
			(w) => w.location.type === "container" && w.location.instance === "mhq-comet"
		);
		expect(workspace?.managed?.state).toEqual({ state: "running" });
		expect(workspace?.managed?.guest.state).toBe("connected");
	});

	it("refuses a workspace on a device without Incus", async () => {
		const made = await source().createWorkspace({
			project: "proj-comet",
			device: "dev-mac",
			name: null,
			start_at_boot: false,
		});
		expect(made).toMatchObject({ status: "ok", data: { status: "refused" } });
	});

	it("sends marking an agent seen as the next delta", async () => {
		const mock = source();
		const deltas: Delta[] = [];
		await mock.subscribe((d) => deltas.push(d));
		const before = applySnapshot(await mock.snapshot());

		expect(await mock.markSeen("agent-usage-popover")).toEqual({ status: "ok", data: null });
		expect(deltas).toHaveLength(1);
		const delta = deltas[0]!;
		expect(delta.seq).toBe((before.seq ?? 0) + 1);
		expect(delta.changes).toHaveLength(1);

		const result = applyDelta(before, delta);
		expect(result.kind).toBe("applied");
		if (result.kind !== "applied") return;
		expect(result.view.tables.agent.get("agent-usage-popover")?.activity.state).toBe("idle");
	});

	it("says when an agent isn't there", async () => {
		expect(await source().markSeen("agent-nope")).toEqual({
			status: "error",
			error: { kind: "not_found", what: "agent agent-nope" },
		});
	});

	it("sends nothing when a change changes nothing", async () => {
		const mock = source();
		const deltas: Delta[] = [];
		await mock.subscribe((d) => deltas.push(d));
		await mock.markSeen("agent-bindings");
		expect(deltas).toHaveLength(0);
	});

	it("switches devbox's setup as a delta and back again", async () => {
		const mock = source();
		const deltas: Delta[] = [];
		await mock.subscribe((d) => deltas.push(d));
		let view = applySnapshot(await mock.snapshot());
		const original = view.tables.device_link.get(DEVBOX_LINK);

		mock.mock.setScenario("peer_offline");
		expect(mock.mock.scenario()).toBe("peer_offline");
		let result = applyDelta(view, deltas[0]!);
		if (result.kind !== "applied") throw new Error(result.kind);
		view = result.view;
		expect(view.tables.device_link.get(DEVBOX_LINK)?.state.state).toBe("unreachable");
		expect(view.tables.agent.get("agent-gtfs")?.activity.state).toBe("not_accessible");
		expect(view.tables.agent.get("agent-usage-popover")?.activity.state).toBe("done_unread");

		mock.mock.setScenario("connected");
		result = applyDelta(view, deltas[1]!);
		if (result.kind !== "applied") throw new Error(result.kind);
		expect(result.view.tables.device_link.get(DEVBOX_LINK)).toEqual(original);
		expect(result.view.tables.agent.get("agent-gtfs")?.activity.state).toBe("busy");
	});

	it("walks a retried device through connecting and back", async () => {
		vi.useFakeTimers();
		const mock = source();
		const states: string[] = [];
		await mock.subscribe((d) => {
			for (const change of d.changes) {
				if (change.op === "upsert" && change.value.entity === "device_link") {
					const s = change.value.state;
					states.push(s.state === "connecting" ? s.step : s.state);
				}
			}
		});
		await mock.refreshLinks(DEVBOX_LINK);
		await vi.runAllTimersAsync();
		expect(states).toEqual(["dialing", "handshake", "connected"]);
	});

	it("moves busy agents along on its own", async () => {
		vi.useFakeTimers();
		let clock = NOW;
		const mock = mockSource({ now: () => clock });
		const deltas: Delta[] = [];
		const stop = await mock.subscribe((d) => deltas.push(d));
		clock += 4000;
		await vi.advanceTimersByTimeAsync(4000);
		expect(deltas).toHaveLength(1);
		const touched = deltas[0]!.changes.map((c) => (c.op === "upsert" ? idOf(c.value) : ""));
		expect(touched).toContain("agent-gtfs");
		expect(touched).not.toContain("agent-bindings");
		stop();
		await vi.advanceTimersByTimeAsync(8000);
		expect(deltas).toHaveLength(1);
	});

	it("opens a sample timeline and says plain actions are done", async () => {
		const mock = source();
		const updates: TimelineUpdate[] = [];
		const close = await mock.openTimeline("agent-gtfs", (u) => updates.push(u));
		expect(updates).toHaveLength(1);
		expect(updates[0]?.type === "reset" && updates[0].items.length).toBeGreaterThan(0);
		close();
		const done = { status: "ok", data: { status: "done" } };
		expect(await mock.interrupt("agent-gtfs")).toEqual(done);
	});

	it("stages attachments like maxd would", async () => {
		const mock = source();
		const [picked] = await mock.stageAttachments("dev-mac", ["/Users/max/shot.png"]);
		expect(picked).toMatchObject({
			status: "ok",
			data: {
				status: "staged",
				attachment: { name: "shot.png", kind: "image", mime: "image/png" },
			},
		});
		const pasted = await mock.stagePasted(
			"dev-mac",
			"notes.pdf",
			"application/pdf",
			new Uint8Array(3)
		);
		expect(pasted).toMatchObject({
			status: "ok",
			data: { status: "staged", attachment: { name: "notes.pdf", kind: "pdf", bytes: 3 } },
		});
	});
});

describe("mock source inboxes", () => {
	const DONE = { status: "ok", data: { status: "done" } };
	const refusal = (reason: string) => ({ status: "ok", data: { status: "refused", reason } });
	const inbox = (view: ViewState, id: string) => inboxOf(view.tables.agent.get(id)!);

	it("has an agent in each place in the inbox", async () => {
		const view = await load();
		expect(inbox(view, "agent-sidebar-sort").pin).not.toBeNull();
		expect(inbox(view, "agent-font-fallback").snoozed_until).toBeGreaterThan(NOW);
		expect(inbox(view, "agent-bindings").woke_at).not.toBeNull();
		expect(inbox(view, "agent-m0-review").settled?.by).toBe("you");
		expect(inbox(view, "agent-license-check").settled?.by).toBe("auto");
	});

	it("applies a change to several agents as one delta", async () => {
		const mock = source();
		const deltas: Delta[] = [];
		await mock.subscribe((d) => deltas.push(d));
		const before = applySnapshot(await mock.snapshot());

		const results = await mock.setInbox(["agent-usage-popover", "agent-bindings"], {
			change: "settle",
		});
		expect(results).toEqual([DONE, DONE]);
		expect(deltas).toHaveLength(1);
		const result = applyDelta(before, deltas[0]!);
		if (result.kind !== "applied") throw new Error("not applied");
		expect(inbox(result.view, "agent-usage-popover").settled).toEqual({ at: NOW, by: "you" });
		expect(inbox(result.view, "agent-bindings").settled).toEqual({ at: NOW, by: "you" });
	});

	it("keeps a change for a device that's away waiting until it's back", async () => {
		const mock = source("peer_offline");
		expect(await mock.setInbox(["agent-gtfs"], { change: "settle" })).toEqual([DONE]);
		let view = applySnapshot(await mock.snapshot());
		expect(view.tables.device_link.get(DEVBOX_LINK)?.waiting).toEqual(["agent-gtfs"]);
		expect(inbox(view, "agent-gtfs").settled).toEqual({ at: NOW, by: "you" });

		mock.mock.setScenario("connected");
		view = applySnapshot(await mock.snapshot());
		expect(view.tables.device_link.get(DEVBOX_LINK)?.waiting).toEqual([]);
		expect(inbox(view, "agent-gtfs").settled).toEqual({ at: NOW, by: "you" });
	});

	it("refuses to settle an agent that needs you, works or has a message waiting", async () => {
		const mock = source();
		const results = await mock.setInbox(
			["agent-release", "agent-m1-lead", "agent-gtfs", "agent-df-chaos", "agent-hdr"],
			{ change: "settle" }
		);
		expect(results).toEqual([
			refusal("the agent needs your input. Answer it before you settle it"),
			refusal("the agent is still working. Stop it or let it finish before you settle it"),
			refusal("the agent is still working. Stop it or let it finish before you settle it"),
			refusal("the agent is still working. Stop it or let it finish before you settle it"),
			refusal(
				"a message is waiting for the agent. Take it back or let it go before you settle it"
			),
		]);
	});

	it("snoozes a busy agent but refuses a time gone by, input or a message waiting", async () => {
		const mock = source();
		const later = { change: "snooze", until: NOW + 60 * 60_000 } as const;
		expect(await mock.setInbox(["agent-gtfs"], later)).toEqual([DONE]);
		expect(await mock.setInbox(["agent-bindings"], { change: "snooze", until: NOW })).toEqual([
			refusal("pick a time in the future to snooze the agent until"),
		]);
		expect(await mock.setInbox(["agent-sync-drift", "agent-hdr"], later)).toEqual([
			refusal("the agent needs your input. Answer it before you snooze it"),
			refusal(
				"a message is waiting for the agent. Take it back or let it go before you snooze it"
			),
		]);
	});

	it("holds an undo that settles again to the same rules", async () => {
		const mock = source();
		const settled = { ...DEFAULT_INBOX, settled: { at: NOW, by: "you" } } as const;
		expect(
			await mock.setInbox(["agent-gtfs", "agent-bindings"], {
				change: "restore",
				inbox: settled,
			})
		).toEqual([
			refusal("the agent is still working. Stop it or let it finish before you settle it"),
			DONE,
		]);
	});

	it("refuses a sub-agent and says when an agent isn't there", async () => {
		const mock = source();
		expect(
			await mock.setInbox(["agent-w1", "agent-nope"], { change: "pin", key: "a1" })
		).toEqual([
			refusal(
				"only lead agents have a place in the inbox. Settle, snooze or pin the agent that started it"
			),
			{ status: "error", error: { kind: "not_found", what: "agent agent-nope" } },
		]);
	});
});

describe("mock source deletes", () => {
	it("deletes a held agent with its prompt, and refuses one from a terminal", async () => {
		const mock = source();
		expect(await mock.deleteAgent("agent-release")).toEqual({
			status: "ok",
			data: { status: "done" },
		});
		const view = applySnapshot(await mock.snapshot());
		expect(view.tables.agent.has("agent-release")).toBe(false);
		expect(view.tables.attention.has("att-release")).toBe(false);

		expect(await mock.deleteAgent("agent-m1-lead")).toEqual({
			status: "ok",
			data: {
				status: "refused",
				reason: "this agent runs outside MaxHQ. Continue it in MaxHQ first",
			},
		});
		expect(await mock.deleteAgent("agent-release")).toEqual({
			status: "error",
			error: { kind: "not_found", what: "agent agent-release" },
		});
	});
});

describe("mock source moves", () => {
	const to = {
		device: DEVBOX,
		account: "acct-claude-nl-token",
		model: null,
		place: { type: "workspace" as const, workspace: "ws-mhq-maxhq" },
	};

	it("refuses an agent in a turn unless it stops it", async () => {
		const mock = source();
		expect(await mock.moveAgent(MAC, "agent-release", MAC, to, false)).toEqual({
			status: "ok",
			data: {
				status: "refused",
				reason: "The agent waits for your answer. Answer it, or stop it and move it.",
			},
		});
		expect(await mock.moveAgent(DEVBOX, "agent-release", MAC, to, true)).toMatchObject({
			data: { reason: "Only the device that runs the MaxHQ app moves agents." },
		});
		expect(
			await mock.moveAgent(MAC, "agent-release", MAC, { ...to, account: CLAUDE }, true)
		).toMatchObject({ data: { reason: "That account isn't on that device." } });
	});

	it("stops the old agent, links both, and points its tasks at the new one", async () => {
		const mock = source();
		const result = await mock.moveAgent(MAC, "agent-release", MAC, to, true);
		if (result.status !== "ok" || result.data.status !== "started") throw new Error("no move");
		const moved = result.data.agent;
		const view = applySnapshot(await mock.snapshot());
		const old = view.tables.agent.get("agent-release");
		expect(old?.activity).toEqual({ state: "paused", reason: "by_you" });
		expect(old?.moved).toEqual({ to: { device: DEVBOX, agent: moved } });
		expect(inboxOf(old!).settled).not.toBeNull();
		expect(view.tables.attention.has("att-release")).toBe(false);
		expect(view.tables.agent.get(moved)).toMatchObject({
			workspace: "ws-mhq-maxhq",
			title: "Release checklist",
			moved: { from: { device: MAC, agent: "agent-release" } },
		});
		const targets = all(view, "device_tasks")
			.flatMap((d) => d.tasks)
			.filter((task) => task.target.type === "agent" && task.target.agent === moved);
		expect(targets.map((task) => task.id).sort()).toEqual(["task-checklist", "task-deploy"]);
		expect(await mock.moveAgent(MAC, "agent-release", MAC, to, true)).toMatchObject({
			data: { reason: "That agent moved already." },
		});
	});
});

describe("mock source renames", () => {
	it("names an agent from a terminal too, and a blank name brings back its title", async () => {
		const mock = source();
		const title = async () =>
			applySnapshot(await mock.snapshot()).tables.agent.get("agent-m1-lead")?.title;
		const before = await title();
		const done = { status: "ok", data: { status: "done" } };
		expect(await mock.renameAgent("agent-m1-lead", `  ${"x".repeat(90)} `)).toEqual(done);
		expect(await title()).toBe("x".repeat(80));
		expect(await mock.renameAgent("agent-m1-lead", "Second")).toEqual(done);
		expect(await title()).toBe("Second");
		expect(await mock.renameAgent("agent-m1-lead", " ")).toEqual(done);
		expect(await title()).toBe(before);
		expect(await mock.renameAgent("agent-nope", "Ghost")).toEqual({
			status: "error",
			error: { kind: "not_found", what: "agent agent-nope" },
		});
	});
});

describe("mock source copies", () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	const toDevbox = {
		type: "folder",
		device: DEVBOX,
		parent: "/home/me/src",
		name: "maxhq",
	} as const;

	it("lists the env files and dev databases git ignores", async () => {
		const { files, truncated } = await source().listIgnoredFiles("ws-mac-maxhq");
		expect(truncated).toBe(false);
		expect(files.map((f) => f.path)).toContain(".env.local");
		expect(files.map((f) => f.path)).toEqual(files.map((f) => f.path).sort());
		await expect(source().listIgnoredFiles("ws-nope")).rejects.toThrow(
			"This workspace isn't there anymore"
		);
	});

	it("clones into a folder through git's steps, then sends the chosen files", async () => {
		vi.useFakeTimers();
		const mock = source();
		const stages: (string | null)[] = [];
		await mock.subscribe((delta) => {
			for (const change of delta.changes) {
				if (change.op !== "upsert" || change.value.entity !== "workspace") continue;
				const clone = change.value.clone?.state;
				stages.push(clone?.state === "cloning" ? clone.stage : null);
			}
		});
		const progress: string[] = [];
		const copied = mock.copyProject("ws-mac-maxhq", toDevbox, [".env", "db/dev.sqlite3"], (p) =>
			progress.push(`${p.file} ${p.sent}`)
		);
		await vi.runAllTimersAsync();
		const { workspace, written, failed } = await copied;
		expect(stages).toEqual([
			"counting",
			"receiving",
			"receiving",
			"receiving",
			"resolving",
			null,
		]);
		expect({ written, failed }).toEqual({ written: 2, failed: [] });
		expect(progress).toEqual([".env 1204", "dev.sqlite3 18874368"]);
		const made = applySnapshot(await mock.snapshot()).tables.workspace.get(workspace);
		expect(made).toMatchObject({
			device: DEVBOX,
			project: "proj-maxhq",
			location: { type: "folder", path: "/home/me/src/maxhq" },
		});
		await expect(mock.copyProject("ws-mac-maxhq", toDevbox, [], () => {})).rejects.toThrow(
			"/home/me/src/maxhq is there already"
		);
	});

	it("stops a cancelled clone and takes it back up on a retry", async () => {
		vi.useFakeTimers();
		const mock = source();
		const copied = mock.copyProject("ws-mac-maxhq", toDevbox, [], () => {});
		const failed = expect(copied).rejects.toThrow("The clone was cancelled");
		const workspace = () =>
			all(applySnapshot(current), "workspace").find(
				(w) => w.location.type === "folder" && w.location.path === "/home/me/src/maxhq"
			)!;
		let current = await mock.snapshot();
		const id = workspace().id;
		expect(await mock.cancelClone(id)).toEqual({ status: "ok", data: { status: "done" } });
		await vi.runAllTimersAsync();
		await failed;
		current = await mock.snapshot();
		expect(workspace().clone).toEqual({ state: { state: "cancelled" } });
		expect(await mock.retryClone(id)).toEqual({ status: "ok", data: { status: "done" } });
		expect(await mock.removeClone(id)).toEqual({
			status: "ok",
			data: { status: "refused", reason: "Cancel the clone before you remove it" },
		});
		await vi.runAllTimersAsync();
		current = await mock.snapshot();
		expect(workspace().clone ?? null).toBeNull();
	});

	it("makes a container that starts at boot and waits for it to run", async () => {
		vi.useFakeTimers();
		const mock = source();
		const copied = mock.copyProject(
			"ws-mac-comet",
			{ type: "container", device: DEVBOX, name: null },
			[".envrc"],
			() => {}
		);
		await vi.runAllTimersAsync();
		const { workspace, written } = await copied;
		const made = applySnapshot(await mock.snapshot()).tables.workspace.get(workspace);
		expect(made?.managed).toMatchObject({
			state: { state: "running" },
			start_at_boot: true,
			guest: { state: "connected" },
		});
		expect(written).toBe(1);
	});
});

describe("mock source SSH setup", () => {
	it("asks for a container, then follows a no and a later yes", async () => {
		const mock = source();
		const alias = "ws-alpha.1a2b3c4d.maxhq";
		const first = await mock.sshInclude("ws-alpha");
		expect(first).toMatchObject({ type: "ask", alias });
		expect(first.type === "ask" && first.line).toMatch(/^Include "/);

		const declined = await mock.setupSshInclude("ws-alpha", false);
		expect(declined).toMatchObject({ type: "declined", alias });
		expect(declined.type === "declined" && declined.block).toContain(`Host ${alias}\n`);
		expect(await mock.sshInclude("ws-alpha")).toEqual(declined);

		expect(await mock.setupSshInclude("ws-alpha", true)).toEqual({ type: "included", alias });
		// One ~/.ssh/config, so every container is included now.
		expect(await mock.sshInclude("ws-bravo")).toEqual({
			type: "included",
			alias: "ws-bravo.1a2b3c4d.maxhq",
		});
	});

	it("says a folder doesn't open this way", async () => {
		const mock = source();
		const { entities } = await mock.snapshot();
		const folder = entities.find(
			(e) => e.entity === "workspace" && e.location.type === "folder"
		);
		if (!folder) throw new Error("the mock should have a folder workspace");
		expect(await mock.sshInclude(idOf(folder))).toMatchObject({ type: "unavailable" });
	});
});

describe("mock source ACP agents", () => {
	it("has one ACP agent on the Mac, with its account and features", async () => {
		const view = await load();
		expect(all(view, "acp_agent").map((a) => [a.device, a.name, a.env])).toEqual([
			[MAC, "Gemini", ["GEMINI_API_KEY"]],
		]);
		const account = view.tables.account.get(GEMINI_ACCOUNT);
		expect(account).toMatchObject({ vendor: "acp", label: "Gemini", login: "acp_command" });
		expect(account?.acp?.auth?.map((m) => m.name)).toEqual([
			"Log in with Google",
			"Use Gemini API key",
		]);
		expect(view.tables.device_link.get("link-mac")?.capabilities?.acp_agents).toBe(true);
	});

	it("saves an agent with its account, and keeps no value", async () => {
		const mock = source();
		const agent = {
			id: "acp-new",
			device: MAC,
			name: "Goose",
			program: "goose",
			args: ["acp"],
		};
		await mock.setAcpAgent(MAC, agent, [{ name: "GOOSE_KEY", value: "s3cret" }]);
		const snapshot = await mock.snapshot();
		expect(JSON.stringify(snapshot)).not.toContain("s3cret");
		const view = applySnapshot(snapshot);
		expect(view.tables.acp_agent.get("acp-new")).toMatchObject({ name: "Goose" });
		expect(view.tables.account.get("acct-acp-new")).toMatchObject({
			vendor: "acp",
			label: "Goose",
			config_dir: "",
		});
	});

	it("removes an agent and its account", async () => {
		const mock = source();
		expect(await mock.deleteAcpAgent(MAC, GEMINI)).toEqual({
			status: "ok",
			data: { status: "done" },
		});
		const view = applySnapshot(await mock.snapshot());
		expect(view.tables.acp_agent.size).toBe(0);
		expect(view.tables.account.has(GEMINI_ACCOUNT)).toBe(false);
	});

	it("refuses to remove one whose agents still run", async () => {
		const mock = source();
		await mock.startAgent({
			vendor: "acp",
			account: GEMINI_ACCOUNT,
			place: { type: "folder", path: "/Users/me/code/app" },
			model: null,
			permission: null,
			title: "Try Gemini",
			first: null,
			branch: null,
		});
		expect(await mock.deleteAcpAgent(MAC, GEMINI)).toEqual({
			status: "ok",
			data: {
				status: "refused",
				reason: "Agents of Gemini are still running. Stop them first.",
			},
		});
	});
});
