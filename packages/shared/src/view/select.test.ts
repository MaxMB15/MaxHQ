// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	account,
	agent,
	attention,
	device,
	entities,
	link,
	MIN,
	network,
	NOW,
	project,
	workspace,
} from "../test/build";

import type { Account, Activity, Agent, Entity, Inbox, Task, WorkflowRun } from "../bindings";

import { DEFAULT_INBOX } from "./inbox";
import { inboxList } from "./inbox-list";
import { applySnapshot } from "./replica";
import {
	accounts,
	byDevice,
	callerOf,
	canHideProjects,
	childrenOf,
	deviceGroups,
	deviceNode,
	fullestContexts,
	hiddenProjectList,
	inboxSections,
	modelName,
	modelsOf,
	movesOf,
	needsYou,
	placeOf,
	projectGroups,
	startedBy,
	startedByLine,
	startedRows,
	waitingFor,
	waitingOnAccount,
} from "./select";

const DAY = 24 * 60 * MIN;

const view = applySnapshot({
	seq: 1,
	entities: entities({
		projects: [project("p-maxhq", "MaxHQ"), project("p-comet", "Comet")],
		devices: [device({ id: "dev-mac", os: "macos" }), device({ id: "dev-nl" })],
		networks: [network()],
		links: [
			link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
			link({ id: "nl", name: "devbox", device: "dev-nl", network: "tailnet" }),
		],
		workspaces: [
			workspace({ id: "w-maxhq", project: "p-maxhq", device: "dev-mac" }),
			workspace({ id: "w-comet-mac", project: "p-comet", device: "dev-mac" }),
			workspace({
				id: "w-comet",
				project: "p-comet",
				device: "dev-nl",
				location: {
					type: "container",
					instance: "ws-bravo",
					path: "/home/agent/Comet",
				},
			}),
			workspace({ id: "w-loose", project: null, device: "dev-mac" }),
		],
		agents: [
			agent({ id: "idle", workspace: "w-maxhq", last_activity_at: NOW - 3 * MIN }),
			agent({ id: "old", workspace: "w-maxhq", last_activity_at: NOW - 3 * DAY }),
			agent({
				id: "old-unread",
				workspace: "w-maxhq",
				activity: { state: "done_unread" },
				last_activity_at: NOW - 3 * DAY,
			}),
			agent({ id: "busy", workspace: "w-maxhq", activity: { state: "busy" } }),
			agent({
				id: "ask",
				workspace: "w-maxhq",
				activity: { state: "needs_input", attention: "att-1" },
			}),
			agent({
				id: "lead",
				workspace: "w-maxhq",
				activity: { state: "busy_in_subagents" },
				context: { used_tokens: 118_000, window_tokens: 200_000 },
			}),
			agent({ id: "sub-b", parent: "lead", workspace: "w-maxhq", started_at: NOW - 2 * MIN }),
			agent({ id: "sub-a", parent: "lead", workspace: "w-maxhq", started_at: NOW - 5 * MIN }),
			agent({
				id: "q",
				workspace: "w-comet",
				activity: { state: "needs_input", attention: "att-2" },
				context: { used_tokens: 182_000, window_tokens: 200_000 },
			}),
			agent({
				id: "down",
				workspace: "w-comet",
				activity: { state: "error", reason: "exited with status 1" },
				link: { state: "down", reason: "exited", since: null },
			}),
			agent({ id: "mac-comet", workspace: "w-comet-mac" }),
			agent({ id: "loose", workspace: "w-loose" }),
		],
		attention: [
			attention({ id: "att-1", agent: "ask", kind: "permission", created_at: NOW - MIN }),
			attention({ id: "att-2", agent: "q", kind: "question", created_at: NOW - 9 * MIN }),
		],
	}),
});

describe("projectGroups", () => {
	const groups = projectGroups(view, NOW);

	it("groups top-level agents by project, by name, with no project last", () => {
		expect(groups.map((g) => g.name)).toEqual(["Comet", "MaxHQ", "No project"]);
		expect(
			groups.flatMap((g) => [...g.agents, ...g.older]).some((a) => a.parent !== null)
		).toBe(false);
	});

	it("sorts needs input, then working, then done unread, then idle", () => {
		const maxhq = groups.find((g) => g.name === "MaxHQ")!;
		expect(maxhq.agents.map((a) => a.id)).toEqual([
			"ask",
			"busy",
			"lead",
			"old-unread",
			"idle",
		]);
	});

	describe("two agents running at once", () => {
		const running = (first: Partial<Agent>, second: Partial<Agent> = {}) => {
			const [group] = projectGroups(
				applySnapshot({
					seq: 1,
					entities: entities({
						workspaces: [workspace({ id: "w", device: "dev-mac" })],
						agents: [
							agent({
								id: "first",
								workspace: "w",
								activity: { state: "busy" },
								started_at: NOW - 2 * MIN,
								last_activity_at: NOW,
								...first,
							}),
							agent({
								id: "second",
								workspace: "w",
								activity: { state: "busy" },
								started_at: NOW - MIN,
								last_activity_at: NOW,
								...second,
							}),
							agent({
								id: "third",
								workspace: "w",
								activity: { state: "idle" },
								last_activity_at: NOW - 30 * MIN,
							}),
						],
					}),
				}),
				NOW
			);
			return group!.agents.map((a) => a.id);
		};

		it("keeps their order whichever ran a command last", () => {
			const order = ["second", "first", "third"];
			expect(running({ last_activity_at: NOW })).toEqual(order);
			expect(running({ last_activity_at: NOW - MIN })).toEqual(order);
			expect(running({ activity: { state: "busy_in_subagents" } })).toEqual(order);
			expect(running({ activity: { state: "starting" } })).toEqual(order);
		});

		it("drops the one that stops below the one still running", () => {
			const stopped = ["first", "second", "third"];
			expect(running({}, { activity: { state: "done_unread" } })).toEqual(stopped);
			expect(running({}, { activity: { state: "idle" } })).toEqual(stopped);
			expect(
				running({}, { activity: { state: "error", reason: "exited with status 1" } })
			).toEqual(stopped);
		});

		it("keeps a running agent's place when a snooze woke it", () => {
			const woken = { inbox: { ...DEFAULT_INBOX, woke_at: NOW } };
			expect(running(woken)).toEqual(["second", "first", "third"]);
		});

		it("still puts one that needs you on top", () => {
			expect(running({ activity: { state: "needs_input", attention: "att" } })).toEqual([
				"first",
				"second",
				"third",
			]);
		});
	});

	it("folds idle agents after a day, but never an unread one", () => {
		const maxhq = groups.find((g) => g.name === "MaxHQ")!;
		expect(maxhq.older.map((a) => a.id)).toEqual(["old"]);
	});

	it("names every device a project runs on", () => {
		expect(groups.find((g) => g.name === "Comet")?.devices).toEqual(["devbox", "Mac"]);
		expect(groups.find((g) => g.name === "MaxHQ")?.devices).toEqual(["Mac"]);
	});
});

describe("a workflow run's node agents", () => {
	const runOf = (run: string, project: string, over: Partial<WorkflowRun>): Entity => ({
		entity: "workflow_run",
		id: `wfr-${run}`,
		key: { device: "dev-mac", project, workflow: "pipeline", run },
		label: run,
		progress: { done: 1, total: 3 },
		current: [],
		needs_you: false,
		orchestrators: ["lead"],
		last_activity: NOW,
		done: false,
		...over,
	});
	const ran = applySnapshot({
		seq: 1,
		entities: [
			...entities({
				projects: [project("p-maxhq", "MaxHQ"), project("p-comet", "Comet")],
				devices: [device({ id: "dev-mac", os: "macos" })],
				workspaces: [workspace({ id: "w", project: "p-maxhq" })],
				agents: [
					agent({ id: "lead", workspace: "w" }),
					agent({ id: "node", workspace: "w" }),
					agent({ id: "was-node", workspace: "w" }),
				],
			}),
			runOf("live", "p-maxhq", { agents: ["node"] }),
			runOf("ended", "p-maxhq", { agents: ["was-node"], done: true }),
			runOf("elsewhere", "p-comet", { orchestrators: [] }),
		],
	});

	it("stay out of the list while their run is live", () => {
		const groups = projectGroups(ran, NOW);
		const listed = groups.flatMap((g) => [...g.agents, ...g.older].map((a) => a.id));
		expect(listed.sort()).toEqual(["lead", "was-node"]);
	});

	it("keep a project with a live run in the list", () => {
		expect(projectGroups(ran, NOW).map((g) => g.name)).toEqual(["Comet", "MaxHQ"]);
	});
});

describe("a dispatcher's agents", () => {
	const homed = applySnapshot({
		seq: 1,
		entities: [
			...entities({
				projects: [project("p-maxhq", "MaxHQ")],
				devices: [device({ id: "dev-mac", os: "macos" })],
				links: [
					link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
				],
				workspaces: [
					workspace({ id: "w", project: "p-maxhq" }),
					workspace({ id: "w-dispatcher" }),
				],
				agents: [
					agent({ id: "plain", workspace: "w" }),
					agent({
						id: "dispatcher",
						workspace: "w-dispatcher",
						activity: { state: "needs_input", attention: "att-d" },
					}),
				],
				attention: [attention({ id: "att-d", agent: "dispatcher", kind: "question" })],
			}),
			{
				entity: "dispatcher",
				device: "dev-mac",
				workspace: "w-dispatcher",
				agent: "dispatcher",
				plan: null,
			},
		],
	});

	it("stay out of the sidebar", () => {
		const listed = projectGroups(homed, NOW).flatMap((g) => g.agents.map((a) => a.id));
		expect(listed).toEqual(["plain"]);
		const sections = inboxSections(homed, NOW);
		expect(Object.values(sections).flat()).toEqual([]);
	});

	it("still need you when they ask", () => {
		expect(needsYou(homed).map((n) => n.agent.id)).toEqual(["dispatcher"]);
	});
});

describe("childrenOf", () => {
	it("lists sub-agents oldest first", () => {
		expect(childrenOf(view, "lead").map((a) => a.id)).toEqual(["sub-a", "sub-b"]);
	});
});

describe("waitingFor", () => {
	it("finds the link an agent's inbox change waits for", () => {
		const waiting = applySnapshot({
			seq: 1,
			entities: entities({
				links: [
					link({ id: "mac" }),
					link({ id: "nl", name: "devbox", waiting: ["a", "b"] }),
				],
			}),
		});
		expect(waitingFor(waiting, "b")?.name).toBe("devbox");
		expect(waitingFor(waiting, "c")).toBeUndefined();
	});
});

describe("needsYou", () => {
	it("lists agents waiting on you, the longest waiting first", () => {
		expect(needsYou(view).map((n) => [n.agent.id, n.ask])).toEqual([
			["q", "Question"],
			["ask", "Allow?"],
		]);
	});
});

describe("byDevice", () => {
	it("splits a project's agents by device, devices by name", () => {
		const comet = projectGroups(view, NOW).find((g) => g.name === "Comet")!;
		expect(
			byDevice(view, comet.agents).map((d) => [d.name, d.agents.map((a) => a.id)])
		).toEqual([
			["devbox", ["q", "down"]],
			["Mac", ["mac-comet"]],
		]);
	});

	it("names a device it can't find", () => {
		const lost = agent({ id: "lost", workspace: "gone" });
		expect(byDevice(view, [lost])).toEqual([
			{ key: "none", name: "Unknown device", agents: [lost] },
		]);
	});
});

describe("deviceGroups", () => {
	const groups = deviceGroups(view, NOW);

	it("puts this Mac first, then each network with its devices and workspaces", () => {
		expect(groups.map((g) => g.name)).toEqual(["This Mac", "Tailnet tail1234"]);
		const nl = groups[1]!.devices[0]!;
		expect(nl.link.name).toBe("devbox");
		expect(nl.workspaces.map((w) => [w.name, w.agents.length])).toEqual([["ws-bravo", 2]]);
	});

	it("shows the worst agent state next to the device's own", () => {
		const nl = groups[1]!.devices[0]!;
		expect(nl.look).toBe("live");
		expect(nl.worst).toBe("agent_down");
		expect(groups[0]!.devices[0]!.worst).toBe("live");
	});

	it("lists devices on a plain SSH path last", () => {
		const extra = applySnapshot({
			seq: 1,
			entities: entities({
				links: [link({ id: "box", device: null, state: { state: "unknown" } })],
			}),
		});
		expect(deviceGroups(extra, NOW).map((g) => [g.name, g.devices[0]?.look])).toEqual([
			["Over SSH", "unknown"],
		]);
	});
});

describe("deviceNode", () => {
	it("is the device as its group lists it", () => {
		const listed = deviceGroups(view, NOW)[1]!.devices[0]!;
		expect(deviceNode(view, listed.link, NOW)).toEqual(listed);
	});
});

describe("the inbox sections", () => {
	const inbox = (over: Partial<Inbox>): Inbox => ({ ...DEFAULT_INBOX, ...over });
	const sorted = applySnapshot({
		seq: 1,
		entities: entities({
			projects: [project("p-maxhq", "MaxHQ")],
			devices: [device({ id: "dev-mac", os: "macos" })],
			links: [link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } })],
			workspaces: [workspace({ id: "w", project: "p-maxhq", device: "dev-mac" })],
			agents: [
				agent({ id: "plain", workspace: "w" }),
				agent({
					id: "busy-old",
					workspace: "w",
					activity: { state: "busy" },
					started_at: NOW - 9 * MIN,
				}),
				agent({
					id: "busy-new",
					workspace: "w",
					activity: { state: "starting" },
					started_at: NOW - MIN,
				}),
				agent({
					id: "pin-b",
					workspace: "w",
					inbox: inbox({ pin: { at: NOW - 5 * MIN, key: "t" } }),
				}),
				agent({
					id: "pin-a",
					workspace: "w",
					inbox: inbox({ pin: { at: NOW, key: "m" } }),
				}),
				agent({
					id: "pin-busy",
					workspace: "w",
					activity: { state: "busy" },
					inbox: inbox({ pin: { at: NOW, key: "z" } }),
				}),
				agent({
					id: "wakes-late",
					workspace: "w",
					inbox: inbox({ snoozed_until: NOW + 90 * MIN }),
				}),
				agent({
					id: "wakes-soon",
					workspace: "w",
					inbox: inbox({ snoozed_until: NOW + 5 * MIN }),
				}),
				agent({
					id: "snooze-over",
					workspace: "w",
					last_activity_at: NOW - 3 * DAY,
					inbox: inbox({ snoozed_until: NOW - MIN, snoozed_at: NOW - DAY }),
				}),
				agent({ id: "woke", workspace: "w", inbox: inbox({ woke_at: NOW - MIN }) }),
				agent({
					id: "woke-old",
					workspace: "w",
					last_activity_at: NOW - 3 * DAY,
					inbox: inbox({ woke_at: NOW - MIN }),
				}),
				agent({
					id: "settled-mine",
					workspace: "w",
					inbox: inbox({ settled: { at: NOW - MIN, by: "you" } }),
				}),
				agent({
					id: "settled-auto",
					workspace: "w",
					inbox: inbox({ settled: { at: NOW - 6 * DAY, by: "auto" } }),
				}),
				agent({
					id: "settled-snoozed",
					workspace: "w",
					inbox: inbox({ settled: { at: NOW, by: "you" }, snoozed_until: NOW + MIN }),
				}),
				agent({
					id: "sub",
					parent: "busy-old",
					workspace: "w",
					inbox: inbox({ pin: { at: NOW, key: "a" } }),
				}),
			],
		}),
	});
	const shelf = { workingShelf: true };

	it("puts snoozed before settled before pinned before working", () => {
		const at = (id: string, workingShelf = true) =>
			placeOf(sorted.tables.agent.get(id)!, NOW, { workingShelf });
		expect(at("settled-snoozed")).toBe("snoozed");
		expect(at("settled-mine")).toBe("settled");
		expect(at("pin-busy")).toBe("pinned");
		expect(at("busy-old")).toBe("working");
		expect(at("busy-old", false)).toBe("active");
		expect(at("snooze-over")).toBe("active");
	});

	it("sorts each section, with sub-agents left to their lead", () => {
		const sections = inboxSections(sorted, NOW, shelf);
		expect(sections.pinned.map((a) => a.id)).toEqual(["pin-a", "pin-b", "pin-busy"]);
		expect(sections.working.map((a) => a.id)).toEqual(["busy-new", "busy-old"]);
		expect(sections.snoozed.map((a) => a.id)).toEqual([
			"settled-snoozed",
			"wakes-soon",
			"wakes-late",
		]);
		expect(sections.settled.map((a) => a.id)).toEqual(["settled-mine", "settled-auto"]);
		expect(inboxSections(sorted, NOW).working).toEqual([]);
	});

	it("leaves only the active agents in the project groups", () => {
		const [group] = projectGroups(sorted, NOW, shelf);
		expect(group!.agents.map((a) => a.id)).toEqual(["woke", "woke-old", "plain"]);
		expect(group!.older.map((a) => a.id)).toEqual(["snooze-over"]);
		const [all] = projectGroups(sorted, NOW);
		expect(all!.agents.map((a) => a.id)).toEqual([
			"busy-new",
			"busy-old",
			"woke",
			"woke-old",
			"plain",
		]);
	});

	it("leaves settled agents out of a device's count and lists them apart", () => {
		const [mac] = deviceGroups(sorted, NOW)[0]!.devices;
		expect(mac!.agents.map((a) => a.id)).not.toContain("settled-mine");
		expect(mac!.settled.map((a) => a.id)).toEqual([
			"settled-snoozed",
			"settled-mine",
			"settled-auto",
		]);
	});
});

describe("usage helpers", () => {
	it("ranks the fullest context windows first", () => {
		expect(fullestContexts(view).map((a) => a.id)).toEqual(["q", "lead"]);
	});

	it("counts agents waiting on an account only while it's at its limit", () => {
		const free = account({ id: "x", vendor: "codex" });
		expect(waitingOnAccount(view, free)).toBe(0);
	});

	it("lists accounts by vendor, Claude to Cursor and then ACP agents, and by name within one", () => {
		const listed = applySnapshot({
			seq: 1,
			entities: entities({
				accounts: [
					account({ id: "gemini", vendor: "acp", label: "Gemini" }),
					account({ id: "cursor", vendor: "cursor", label: "Cursor" }),
					account({ id: "work", label: "Work" }),
					account({ id: "codex", vendor: "codex", label: "Codex" }),
					account({ id: "home", label: "Home" }),
					account({ id: "amp", vendor: "acp", label: "Amp" }),
				],
			}),
		});
		expect(accounts(listed).map((a) => a.id)).toEqual([
			"home",
			"work",
			"codex",
			"cursor",
			"amp",
			"gemini",
		]);
	});
});

describe("modelsOf", () => {
	const CLAUDE = [
		{ id: "default", label: "Default (recommended)", default: true },
		{ id: "opus", label: "Opus", default: false },
		{ id: "sonnet", label: "Sonnet", default: false },
	];
	const CODEX = [{ id: "gpt-5.5", label: "GPT-5.5", default: true }];
	const mac = account({ id: "a-mac", plan: "max", models: CLAUDE });
	// The devbox's setup-token login hasn't run a session yet.
	const fresh = account({ id: "a-token", device: "dev-nl", plan: "max" });
	const codex = account({ id: "a-codex", vendor: "codex", label: "Codex", plan: "plus" });
	const cursor = account({ id: "a-cursor", vendor: "cursor", label: "Cursor", plan: "pro" });
	const viewOf = (...accounts: Account[]) =>
		applySnapshot({ seq: 1, entities: entities({ accounts }) });

	it("keeps an account's own list", () => {
		const own = { ...fresh, models: CLAUDE.slice(1) };
		expect(modelsOf(viewOf(mac, own), own)).toEqual(CLAUDE.slice(1));
	});

	it("borrows the same vendor's longest list while its own is empty", () => {
		const short = account({ id: "a-a", models: CLAUDE.slice(0, 1) });
		const tied = account({ id: "a-z", models: CLAUDE });
		expect(modelsOf(viewOf(short, tied, mac, fresh), fresh)).toBe(mac.models);
	});

	it("never borrows from another vendor", () => {
		const view = viewOf(mac, codex, cursor);
		expect(modelsOf(view, codex)).toEqual([]);
		expect(modelsOf(view, cursor)).toEqual([]);
		const lender = { ...codex, id: "a-codex-2", models: CODEX };
		expect(modelsOf(viewOf(mac, codex, lender), codex)).toBe(lender.models);
	});

	it("has nothing for no account", () => {
		expect(modelsOf(viewOf(mac), undefined)).toEqual([]);
	});
});

describe("movesOf", () => {
	const CODEX = [{ id: "gpt-5.5", label: "GPT-5.5", default: true }];
	const claude = account({ id: "a-claude" });
	const codex = account({ id: "a-codex", vendor: "codex", label: "Codex", models: CODEX });
	const cursor = account({ id: "a-cursor", vendor: "cursor", label: "Cursor" });
	const gemini = account({ id: "a-gemini", vendor: "acp", label: "Gemini" });
	const qwen = account({ id: "a-qwen", vendor: "acp", label: "Qwen" });
	const claude2 = account({ id: "a-claude-2", label: "Claude work" });
	const devbox = account({ id: "a-devbox", vendor: "codex", device: "dev-nl" });
	const held = agent({ id: "held", control: "held", account: "a-claude", workspace: "w-mac" });
	const viewOf = (...agents: Agent[]) =>
		applySnapshot({
			seq: 1,
			entities: entities({
				workspaces: [workspace({ id: "w-mac" })],
				accounts: [claude, codex, cursor, gemini, qwen, claude2, devbox],
				agents,
			}),
		});
	const ids = (agent: Agent) => movesOf(viewOf(agent), agent).map((m) => m.account.id);

	it("offers the other vendors signed in on the agent's device, with their models", () => {
		const moves = movesOf(viewOf(held), held);
		expect(moves.map((m) => m.account.id)).toEqual([
			"a-codex",
			"a-cursor",
			"a-gemini",
			"a-qwen",
		]);
		expect(moves.find((m) => m.account.id === "a-codex")?.models).toBe(codex.models);
	});

	it("lets an ACP agent move to another ACP agent", () => {
		const acp = { ...held, vendor: "acp" as const, account: "a-gemini" };
		expect(ids(acp)).toEqual(["a-claude", "a-claude-2", "a-codex", "a-cursor", "a-qwen"]);
	});

	it("offers nothing to an agent MaxHQ doesn't hold, or to a sub-agent", () => {
		expect(ids({ ...held, control: "observed" })).toEqual([]);
		expect(ids({ ...held, parent: "lead" })).toEqual([]);
	});

	it("names a model by its vendor's label for it", () => {
		const view = viewOf(held);
		expect(modelName(view, "codex", "gpt-5.5")).toBe("GPT-5.5");
		expect(modelName(view, "claude", "gpt-5.5")).toBe("gpt-5.5");
	});
});

const mockTask: Task = {
	id: "task",
	name: "Task",
	prompt: "p",
	trigger: { type: "every", minutes: 15 },
	target: { type: "agent", device: "dev-mac", agent: "caller" },
	enabled: true,
	permission: "perm-1",
	made_by: { type: "user" },
	next_run: null,
	last_run: null,
};

describe("agents started through the agent tools", () => {
	const origin = (by: string | null) => ({
		permission: "perm-1",
		by:
			by === null
				? { kind: "key" as const, key: "key-1" }
				: { kind: "agent" as const, agent: by },
		device: "dev-mac",
	});
	const started = applySnapshot({
		seq: 1,
		entities: entities({
			devices: [device({ id: "dev-mac", os: "macos" })],
			links: [link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } })],
			workspaces: [workspace({ id: "w1", project: "p", device: "dev-mac" })],
			projects: [project("p", "MaxHQ")],
			agents: [
				agent({ id: "caller", title: "Release checklist" }),
				agent({ id: "late", origin: origin("caller"), started_at: NOW - 5 * MIN }),
				agent({ id: "early", origin: origin("caller"), started_at: NOW - 10 * MIN }),
				agent({
					id: "pinned",
					origin: origin("caller"),
					inbox: { ...DEFAULT_INBOX, pin: { at: NOW, key: "a0" } },
				}),
				agent({ id: "orphan", origin: origin("gone") }),
				agent({ id: "by-key", origin: origin(null) }),
				agent({ id: "grandchild", origin: origin("early") }),
			],
		}),
	});

	it("shows a started agent under its caller, oldest first", () => {
		expect(startedBy(started, "caller", NOW).map((a) => a.id)).toEqual(["early", "late"]);
		const rows = projectGroups(started, NOW)[0]!.agents.map((a) => a.id);
		expect(rows).not.toContain("early");
		expect(rows).toContain("orphan");
		expect(rows).toContain("by-key");
	});

	it("lists every level under a caller in row order", () => {
		const rows = startedRows(started, "caller", NOW).map((r) => `${r.agent.id}:${r.level}`);
		expect(rows).toEqual(["early:1", "grandchild:2", "late:1"]);
	});

	it("keeps it in the lists when you pinned it or the caller is gone", () => {
		const late = started.tables.agent.get("late")!;
		expect(callerOf(started, late, NOW)?.id).toBe("caller");
		expect(callerOf(started, started.tables.agent.get("pinned")!, NOW)).toBeUndefined();
		expect(callerOf(started, started.tables.agent.get("orphan")!, NOW)).toBeUndefined();
		expect(inboxSections(started, NOW).pinned.map((a) => a.id)).toEqual(["pinned"]);
	});

	it("names who started it and where", () => {
		const line = (id: string) => startedByLine(started, started.tables.agent.get(id)!);
		expect(line("late")).toBe("Started by Release checklist on Mac");
		expect(line("by-key")).toBe("Started by an access key on Mac");
		expect(line("caller")).toBeUndefined();
	});

	it("names the scheduled task that started it", () => {
		const byTask = (task: string) =>
			agent({
				id: `by-${task}`,
				origin: { permission: "perm-1", by: { kind: "task", task }, device: "dev-mac" },
			});
		const view = applySnapshot({
			seq: 1,
			entities: [
				...entities({
					devices: [device({ id: "dev-mac", os: "macos" })],
					links: [
						link({
							id: "mac",
							name: "Mac",
							device: "dev-mac",
							route: { type: "local" },
						}),
					],
					workspaces: [workspace({ id: "w1", project: "p", device: "dev-mac" })],
					agents: [byTask("task-1"), byTask("task-gone")],
				}),
				{
					entity: "device_tasks",
					device: "dev-mac",
					tasks: [{ ...mockTask, id: "task-1", name: "Morning review" }],
					funnel: { state: "off" },
					problems: [],
				},
			],
		});
		const line = (id: string) => startedByLine(view, view.tables.agent.get(id)!);
		expect(line("by-task-1")).toBe("Started by the task Morning review on Mac");
		expect(line("by-task-gone")).toBe("Started by a scheduled task on Mac");
	});
});

describe("hidden projects", () => {
	const hiding = (agents: Agent[], lists: Record<string, string[]> = { "dev-mac": ["p-a"] }) =>
		applySnapshot({
			seq: 1,
			entities: [
				...entities({
					projects: [project("p-a", "Scratch"), project("p-b", "Billing")],
					workspaces: [
						workspace({ id: "w-a", project: "p-a" }),
						workspace({ id: "w-b", project: "p-b" }),
					],
					agents: [agent({ id: "b", workspace: "w-b" }), ...agents],
				}),
				...Object.entries(lists).map(([device, projects]) => ({
					entity: "hidden_projects" as const,
					device,
					projects,
				})),
			],
		});
	const shown = (agents: Agent[], lists?: Record<string, string[]>) =>
		projectGroups(hiding(agents, lists), NOW).map(
			(g) => `${g.key}${g.hidden ? " hidden" : ""}`
		);
	const doing = (activity: Activity) => agent({ id: "a", workspace: "w-a", activity });

	it("leaves the sidebar while none of its agents calls it back", () => {
		expect(shown([agent({ id: "a", workspace: "w-a" })])).toEqual(["p-b"]);
		expect(shown([agent({ id: "a", workspace: "w-a" })], {})).toEqual(["p-b", "p-a"]);
	});

	it("hides a project any device's list names", () => {
		const lists = { "dev-mac": [], "dev-nl": ["p-a", "p-b"] };
		expect(shown([agent({ id: "a", workspace: "w-a" })], lists)).toEqual([]);
	});

	it("shows again while an agent is busy, queued, needs you, is unread or failed", () => {
		const back: Activity[] = [
			{ state: "starting" },
			{ state: "busy" },
			{ state: "busy_in_subagents" },
			{ state: "waiting_for_limit_reset", resets_at: null },
			{ state: "needs_input", attention: "att" },
			{ state: "done_unread" },
			{ state: "error", reason: "exited with status 1" },
		];
		for (const activity of back) {
			expect(shown([doing(activity)]), activity.state).toEqual(["p-b", "p-a hidden"]);
		}
		const woke = agent({
			id: "a",
			workspace: "w-a",
			inbox: { ...DEFAULT_INBOX, woke_at: NOW },
		});
		expect(shown([woke])).toEqual(["p-b", "p-a hidden"]);
	});

	it("hides again once its agents settle down", () => {
		const quiet: Activity[] = [
			{ state: "idle" },
			{ state: "idle_cache_warm", warm_until: NOW + MIN },
			{ state: "paused", reason: "by_you" },
			{ state: "not_accessible" },
		];
		for (const activity of quiet) {
			expect(shown([doing(activity)]), activity.state).toEqual(["p-b"]);
		}
	});

	it("shows again while an agent shown under one of its agents calls it back", () => {
		const caller = agent({ id: "a", workspace: "w-a" });
		const started = agent({
			id: "started",
			workspace: "w-b",
			activity: { state: "needs_input", attention: "att" },
			origin: { permission: "perm-1", by: { kind: "agent", agent: "a" }, device: "dev-mac" },
		});
		expect(shown([caller, started])).toEqual(["p-b", "p-a hidden"]);
	});

	it("keeps an agent that needs you in the Needs you box", () => {
		const view = hiding([doing({ state: "needs_input", attention: "att" })]);
		expect(needsYou(view).map((item) => item.agent.id)).toEqual(["a"]);
	});

	it("lists the hidden projects by name, once each", () => {
		const view = hiding([], { "dev-mac": ["p-a", "gone"], "dev-nl": ["p-b", "p-a"] });
		expect(hiddenProjectList(view).map((p) => p.name)).toEqual(["Billing", "Scratch"]);
		expect(canHideProjects(view)).toBe(true);
		expect(canHideProjects(hiding([], {}))).toBe(false);
	});
});

describe("projects with notes", () => {
	const summary = (note: string, deleted = false) => ({
		note,
		title: note,
		head: `head-${note}`,
		at: NOW,
		by: null,
		created_on: "dev-mac",
		deleted,
		size: 1,
	});
	const withNotes = (notes: Record<string, boolean>, hidden: string[] = []) =>
		applySnapshot({
			seq: 1,
			entities: [
				...entities({
					projects: [
						project("p-a", "Atlas"),
						project("p-b", "Billing"),
						project("p-c", "Comet"),
					],
					workspaces: [workspace({ id: "w-b", project: "p-b" })],
					agents: [agent({ id: "b", workspace: "w-b" })],
				}),
				...Object.entries(notes).map(([proj, deleted]) => ({
					entity: "project_notes" as const,
					device: "dev-mac",
					project: proj,
					notes: [summary(`n-${proj}`, deleted)],
				})),
				{ entity: "hidden_projects" as const, device: "dev-mac", projects: hidden },
			],
		});
	const groups = (notes: Record<string, boolean>, hidden?: string[]) =>
		projectGroups(withNotes(notes, hidden), NOW).map(
			(g) => `${g.key} ${g.agents.length + g.older.length}`
		);

	it("gives a project with live notes a group with no agents", () => {
		expect(groups({ "p-a": false })).toEqual(["p-a 0", "p-b 1"]);
	});

	it("leaves out a project whose notes are all deleted", () => {
		expect(groups({ "p-c": true })).toEqual(["p-b 1"]);
	});

	it("keeps one group for a project with notes and agents", () => {
		expect(groups({ "p-b": false })).toEqual(["p-b 1"]);
	});

	it("keeps a hidden project hidden though it has notes", () => {
		expect(groups({ "p-a": false }, ["p-a"])).toEqual(["p-b 1"]);
	});

	it("gives a project with only notes an empty group in the phone's inbox", () => {
		const list = inboxList(withNotes({ "p-a": false }), NOW);
		expect(list.groups.map((g) => [g.key, g.project, g.rows.length])).toEqual([
			["p-a", "p-a", 0],
			["p-b", "p-b", 1],
		]);
	});
});
