// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Activity, Agent, DeviceLink, Entity, Terminal } from "../bindings";
import { account, agent, attention, entities, link, MIN, NOW, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";

import {
	ACTIVITY_LABEL,
	activityCard,
	activityLook,
	agentLook,
	agentRowMeta,
	rowMeta,
	subAgentStatus,
	type ActivityLook,
} from "./activity";

const HOUR = 60 * MIN;

const cases: [Activity, ActivityLook, string][] = [
	[{ state: "starting" }, "starting", "Starting"],
	[{ state: "busy" }, "busy", "Busy"],
	[{ state: "busy_in_subagents" }, "busy_in_subagents", "Waiting on sub-agents"],
	[{ state: "needs_input", attention: "q1" }, "needs_input", "Needs input"],
	[{ state: "done_unread" }, "done_unread", "Done, not opened yet"],
	[
		{ state: "idle_cache_warm", warm_until: NOW + 38 * MIN },
		"idle_cache_warm",
		"Idle, cache warm",
	],
	[{ state: "idle" }, "idle", "Idle"],
	[
		{ state: "waiting_for_limit_reset", resets_at: null },
		"waiting_for_limit_reset",
		"Waiting for a usage limit",
	],
	[{ state: "paused", reason: "by_you" }, "paused", "Paused"],
	[{ state: "paused", reason: "resumes_made_no_progress" }, "paused", "Paused after 3 resumes"],
	[{ state: "paused", reason: "resume_off" }, "paused", "Paused after a restart"],
	[{ state: "error", reason: "exited with status 1" }, "error", "Stopped with an error"],
	[{ state: "not_accessible" }, "not_accessible", "Not accessible"],
];

describe("activity dot", () => {
	it.each(cases)("maps %j to its look, label and hover", (activity, look, title) => {
		expect(activityLook(activity, NOW)).toBe(look);
		expect(ACTIVITY_LABEL[look]).toBeTruthy();
		const card = activityCard({
			agent: agent({ id: "a", activity, started_at: NOW - 5000 }),
			now: NOW,
		});
		expect(card.title).toBe(title);
		expect(card.body ?? card.foot).toBeTruthy();
	});

	it("covers every state", () => {
		// The two looks an agent's running jobs add, tested below.
		const looks = new Set<ActivityLook>([
			...cases.map(([, look]) => look),
			"idle_running",
			"idle_waiting",
		]);
		expect([...looks].sort()).toEqual(Object.keys(ACTIVITY_LABEL).sort());
	});

	it("shows a warm cache past its time as idle", () => {
		const activity: Activity = { state: "idle_cache_warm", warm_until: NOW - 1 };
		expect(activityLook(activity, NOW)).toBe("idle");
		expect(activityCard({ agent: agent({ id: "a", activity }), now: NOW }).title).toBe("Idle");
	});
});

describe("activity hover", () => {
	it("says a message brings back an agent you stopped, as the Stopped banner does", () => {
		for (const reason of ["by_you", "resume_off"] as const) {
			const activity: Activity = { state: "paused", reason };
			const card = activityCard({ agent: agent({ id: "a", activity }), now: NOW });
			expect(card.body).toContain(
				"Resume starts it again, and so does sending it a message."
			);
		}
	});

	it("says when Claude's cache goes cold, as a fact", () => {
		const card = activityCard({
			agent: agent({
				id: "a",
				activity: { state: "idle_cache_warm", warm_until: NOW + 38 * MIN },
				context: { used_tokens: 118_000, window_tokens: 200_000 },
			}),
			now: NOW,
		});
		expect(card.body).toContain("in 38 min");
		expect(card.body).not.toContain("estimate");
		expect(card.facts).toContainEqual({ label: "Context", value: "118k of 200k, cache warm" });
	});

	it("calls Codex warmth an estimate", () => {
		const card = activityCard({
			agent: agent({
				id: "a",
				vendor: "codex",
				activity: { state: "idle_cache_warm", warm_until: NOW + 4 * MIN },
				context: { used_tokens: 64_000, window_tokens: 272_000 },
			}),
			now: NOW,
		});
		expect(card.body).toContain("estimate");
		expect(card.facts).toContainEqual({
			label: "Context",
			value: "64k of 272k, cache warm, estimate",
		});
	});

	it("names a local agent Local with its model, not by a login", () => {
		const local = account({ id: "l", login: "model_server", model_server: "srv" });
		const card = activityCard({
			agent: agent({ id: "a", account: "l", local: {}, activity: { state: "busy" } }),
			account: local,
			localLabel: "Local · Qwen3.6-35B-A3B",
			now: NOW,
		});
		expect(card.facts).toContainEqual({ label: "Agent", value: "Local · Qwen3.6-35B-A3B" });
		expect(card.facts.map((f) => f.label)).not.toContain("Account");
		expect(JSON.stringify(card)).not.toContain("Claude");
	});

	it("names the prompt and where to answer it", () => {
		const card = activityCard({
			agent: agent({ id: "a", activity: { state: "needs_input", attention: "p1" } }),
			attention: attention({ id: "p1", agent: "a", summary: "Run incus restart ws-bravo" }),
			now: NOW,
		});
		expect(card.title).toBe("Asking for permission");
		expect(card.body).toBe("Run incus restart ws-bravo");
		expect(card.foot).toContain("terminal");

		const question = activityCard({
			agent: agent({ id: "a", activity: { state: "needs_input", attention: "q1" } }),
			attention: attention({ id: "q1", agent: "a", kind: "question" }),
			now: NOW,
		});
		expect(question.title).toBe("Asking you a question");
	});

	it("says which account and which limit an agent waits for", () => {
		const resets = NOW + 2 * HOUR + 22 * MIN;
		const codex = account({
			id: "x",
			vendor: "codex",
			plan: "plus",
			usage: {
				checked_at: NOW,
				source: "codex_app_server",
				windows: [
					{ scope: null, window_minutes: 300, used_percent: 100, resets_at: resets },
					{
						scope: null,
						window_minutes: 10080,
						used_percent: 64,
						resets_at: NOW + 72 * HOUR,
					},
				],
			},
		});
		const card = activityCard({
			agent: agent({
				id: "a",
				vendor: "codex",
				account: "x",
				activity: { state: "waiting_for_limit_reset", resets_at: resets },
			}),
			account: codex,
			now: NOW,
		});
		expect(card.title).toBe("Waiting for Codex Plus");
		expect(card.body).toBe(
			"The 5-hour limit resets at 17:10, in 2 h 22 min. The agent sends its next turn then, with nothing lost."
		);
		expect(card.facts).toContainEqual({
			label: "Account",
			value: "Codex Plus, at its 5-hour limit",
		});
	});

	it("lists the sub-agents a lead waits on", () => {
		const card = activityCard({
			agent: agent({ id: "lead", activity: { state: "busy_in_subagents" } }),
			children: [
				agent({
					id: "s1",
					title: "df-testing",
					parent: "lead",
					activity: { state: "busy" },
				}),
				agent({
					id: "s2",
					title: "df-security",
					parent: "lead",
					activity: { state: "busy" },
				}),
				agent({ id: "s3", title: "df-perf", parent: "lead", activity: { state: "idle" } }),
			],
			now: NOW,
		});
		expect(card.title).toBe("Waiting on 2 sub-agents");
		expect(card.body).toContain("df-testing and df-security are still working");
	});

	it("tells a resume from a fresh launch", () => {
		const resuming = agent({
			id: "a",
			activity: { state: "starting" },
			started_at: NOW - 3 * HOUR,
		});
		expect(activityCard({ agent: resuming, now: NOW }).title).toBe("Resuming after a restart");
		const fresh = agent({ id: "a", activity: { state: "starting" }, started_at: NOW - 5000 });
		expect(activityCard({ agent: fresh, now: NOW }).title).toBe("Starting");
	});
});

describe("row meta", () => {
	it("shows the pill, the sub-agent count, the reset time or how long ago", () => {
		expect(
			rowMeta(agent({ id: "a", activity: { state: "needs_input", attention: "p" } }), [], NOW)
		).toEqual({
			kind: "input",
		});
		const kids = [
			agent({ id: "s1", parent: "a", activity: { state: "busy" } }),
			agent({ id: "s2", parent: "a", activity: { state: "busy" } }),
			agent({ id: "s3", parent: "a", activity: { state: "starting" } }),
		];
		expect(
			rowMeta(agent({ id: "a", activity: { state: "busy_in_subagents" } }), kids, NOW)
		).toEqual({
			kind: "text",
			text: "3 sub",
		});
		const resets = new Date(2026, 9, 25, 17, 10).getTime();
		expect(
			rowMeta(
				agent({
					id: "a",
					activity: { state: "waiting_for_limit_reset", resets_at: resets },
				}),
				[],
				NOW
			)
		).toEqual({ kind: "text", text: "17:10" });
		expect(rowMeta(agent({ id: "a", last_activity_at: NOW - 4 * MIN }), [], NOW)).toEqual({
			kind: "text",
			text: "4m",
		});
		expect(
			rowMeta(
				agent({ id: "a", activity: { state: "starting" }, started_at: NOW - HOUR }),
				[],
				NOW
			)
		).toEqual({ kind: "text", text: "Resuming" });
	});

	describe("an agent whose idle CLI stopped", () => {
		const asleep = { kind: "asleep", text: "Asleep" };
		const ago = { kind: "text", text: "4m" };
		const sleeper = (over: Partial<Agent> = {}) =>
			agent({
				id: "a",
				workspace: "w",
				asleep: true,
				last_activity_at: NOW - 4 * MIN,
				...over,
			});
		const metaOn = (shown: Agent, devbox: DeviceLink = link({ id: "devbox" })) =>
			agentRowMeta(
				applySnapshot({
					seq: 1,
					entities: entities({
						agents: [shown],
						links: [devbox],
						workspaces: [workspace({ id: "w", device: "dev-devbox" })],
					}),
				}),
				shown,
				NOW
			);

		it("says it's asleep in place of its time while it's quiet", () => {
			expect(metaOn(sleeper())).toEqual(asleep);
			const warm = { state: "idle_cache_warm", warm_until: NOW - MIN } as const;
			expect(metaOn(sleeper({ activity: warm }))).toEqual(asleep);
			expect(metaOn(sleeper({ asleep: false }))).toEqual(ago);
		});

		it("shows unread first, and other states as they are", () => {
			expect(metaOn(sleeper({ activity: { state: "done_unread" } }))).toEqual(ago);
			const paused = { state: "paused", reason: "by_you" } as const;
			expect(metaOn(sleeper({ activity: paused }))).toEqual({ kind: "text", text: "Paused" });
		});

		it("lets the device's state win", () => {
			const offline = link({
				id: "devbox",
				state: { state: "unreachable", reason: "timed out", last_seen: NOW - 9 * MIN },
			});
			expect(metaOn(sleeper(), offline)).toEqual(ago);
			const gone = { state: "down", reason: "exited", since: null } as const;
			expect(metaOn(sleeper({ link: gone }))).toEqual(ago);
		});

		it("follows the look it's given", () => {
			const quiet = sleeper();
			expect(rowMeta(quiet, [], NOW, "asleep")).toEqual(asleep);
			expect(rowMeta(quiet, [], NOW, "connecting")).toEqual(ago);
			expect(rowMeta(quiet, [], NOW)).toEqual(ago);
		});
	});

	it("says Input while a command you ran waits for you, whatever the agent does", () => {
		const busy = agent({ id: "a", activity: { state: "busy" }, started_at: NOW - HOUR });
		const run = (over: Partial<Terminal>): Entity => ({
			entity: "terminal",
			id: "t",
			agent: "a",
			device: "dev-mac",
			kind: { type: "run", command: "ssh devbox", private: false },
			moved: false,
			title: "ssh devbox",
			cwd: "~",
			status: { state: "running" },
			password_prompt: false,
			started_at: NOW - MIN,
			cols: 120,
			rows: 32,
			...over,
		});
		const metaWith = (terminal: Entity) =>
			agentRowMeta(
				applySnapshot({ seq: 1, entities: [...entities({ agents: [busy] }), terminal] }),
				busy,
				NOW
			);
		expect(metaWith(run({ waiting: true, choices: ["yes", "no"] }))).toEqual({ kind: "input" });
		expect(metaWith(run({ password_prompt: true }))).toEqual({ kind: "input" });
		expect(metaWith(run({}))).not.toEqual({ kind: "input" });
	});

	it("names every sub-agent state", () => {
		for (const [activity] of cases) expect(subAgentStatus(activity)).toBeTruthy();
	});
});

describe("what an idle agent left running", () => {
	const idle: Activity = { state: "idle" };
	const two = { count: 2, names: ["pnpm dev", "cargo watch"] };

	it("looks as before when maxd sends nothing, as an older one does", () => {
		const a = agent({ id: "a", activity: idle });
		expect(agentLook(a, NOW)).toBe("idle");
		expect(activityCard({ agent: a, now: NOW }).title).toBe("Idle");
		const none = agent({ id: "b", activity: idle, running: { count: 0, names: [] } });
		expect(agentLook(none, NOW)).toBe("idle");
	});

	it("says which processes still run once its turn is done", () => {
		const a = agent({ id: "a", activity: idle, running: two });
		expect(agentLook(a, NOW)).toBe("idle_running");
		const card = activityCard({ agent: a, now: NOW });
		expect(card.title).toBe("Done, processes still running");
		expect(card.body).toBe("2 processes still running: pnpm dev and cargo watch.");
	});

	it("counts the processes it doesn't name", () => {
		const a = agent({
			id: "a",
			activity: { state: "idle_cache_warm", warm_until: NOW + 5 * MIN },
			running: { count: 4, names: ["pnpm dev", "cargo watch"] },
		});
		expect(agentLook(a, NOW)).toBe("idle_running");
		expect(activityCard({ agent: a, now: NOW }).body).toBe(
			"4 processes still running: pnpm dev, cargo watch and 2 more."
		);
	});

	it("says it waits on a background task, with what else runs", () => {
		const a = agent({
			id: "a",
			activity: idle,
			running: { ...two, waiting: { kind: "task", name: "pnpm test" } },
		});
		expect(agentLook(a, NOW)).toBe("idle_waiting");
		const card = activityCard({ agent: a, now: NOW });
		expect(card.title).toBe("Waiting on a background task");
		expect(card.body).toContain("pnpm test");
		expect(card.facts).toContainEqual({
			label: "Still running",
			value: "pnpm dev and cargo watch",
		});
	});

	it("waits on a wakeup until it's due", () => {
		const wakeup = (at: number) =>
			agent({
				id: "a",
				activity: idle,
				running: { count: 0, names: [], waiting: { kind: "wakeup", at } },
			});
		const soon = wakeup(NOW + 10 * MIN);
		expect(agentLook(soon, NOW)).toBe("idle_waiting");
		const card = activityCard({ agent: soon, now: NOW });
		expect(card.title).toBe("Waiting on a wakeup");
		expect(card.body).toContain("10 min");
		expect(agentLook(wakeup(NOW - 1), NOW)).toBe("idle");
	});

	it("never takes the place of a state that needs you, or of work", () => {
		const running = { ...two, waiting: { kind: "task" as const, name: "pnpm test" } };
		const states: Activity[] = [
			{ state: "needs_input", attention: "q1" },
			{ state: "done_unread" },
			{ state: "busy" },
		];
		for (const activity of states) {
			expect(agentLook(agent({ id: "a", activity, running }), NOW)).toBe(activity.state);
		}
	});

	it("lists the processes on a turn you haven't read", () => {
		const a = agent({ id: "a", activity: { state: "done_unread" }, running: two });
		expect(activityCard({ agent: a, now: NOW }).facts).toContainEqual({
			label: "Still running",
			value: "pnpm dev and cargo watch",
		});
	});
});
