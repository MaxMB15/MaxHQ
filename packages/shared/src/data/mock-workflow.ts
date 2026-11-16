// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The workflow runs of the sample data and of tests, in two placeholder
// projects. They mirror a real setup: Claude on this Mac, Codex on devbox
// and Cursor in a container. Each run is a `workflow_run` entity the way
// `maxd` publishes it, plus the snapshot an open of it starts with.

import type {
	Agent,
	AgentId,
	AttemptSpan,
	Badge,
	DetailList,
	EdgeShape,
	Entity,
	GroupShape,
	LoopState,
	NodeShape,
	NodeState,
	NodeStatus,
	Project,
	RunKey,
	RunNotice,
	RunSnapshot,
	Timestamp,
	WorkflowRun,
	Workspace,
} from "../bindings";
import { runName } from "../workflow/select";
import { CLAUDE, DEVBOX, DEVBOX_CODEX, MAC, seed, type AgentSeed } from "./mock-view";

const MIN = 60_000;

/** The placeholder project with a pipeline descriptor. */
export const EXAMPLE_APP = "proj-example-app";
/** The placeholder project with a run no agent leads, and a cr run. */
export const EXAMPLE_API = "proj-example-api";

const project = (id: string, name: string): Project => ({
	id,
	name,
	repo: `github.com/owner/${name}`,
	default_vendor: "claude",
	default_account: null,
	default_workspace: null,
});

const APP_MAC = "ws-mac-example-app";
const API_MAC = "ws-mac-example-api";
const APP_BOX = "ws-example-app";
const APP_CURSOR = "ws-example-app-cursor";

const workspaces = (): Workspace[] => [
	{
		id: APP_MAC,
		project: EXAMPLE_APP,
		device: MAC,
		location: { type: "folder", path: "/Users/me/Code/example-app" },
	},
	{
		id: API_MAC,
		project: EXAMPLE_API,
		device: MAC,
		location: { type: "folder", path: "/Users/me/Code/example-api" },
	},
	{
		id: APP_BOX,
		project: EXAMPLE_APP,
		device: DEVBOX,
		location: { type: "container", instance: APP_BOX, path: "/home/agent/example-app" },
	},
	{
		id: APP_CURSOR,
		project: EXAMPLE_APP,
		device: DEVBOX,
		location: { type: "container", instance: APP_CURSOR, path: "/home/agent/example-app" },
	},
];

/** The leads of the sample runs, by run. */
export const RUN_LEADS = {
	analysis: "agent-ex-analysis",
	approve: "agent-ex-approve",
	codex: "agent-ex-codex",
	loops: "agent-ex-loops",
	journal: "agent-ex-journal",
	agents: "agent-ex-agents",
	cr: "agent-ex-cr",
} as const satisfies Record<string, AgentId>;

const agents = (now: Timestamp): Agent[] => {
	const onMac = (workspace: string, s: AgentSeed): Agent =>
		seed(now, {
			workspace,
			account: CLAUDE,
			pid: 61_000,
			started_at: now - 2 * 60 * MIN,
			...s,
		});
	const app = (s: AgentSeed) => onMac(APP_MAC, s);
	const sub = (parent: AgentId, id: string, title: string, busy: boolean): Agent =>
		app({
			id,
			parent,
			title,
			activity: { state: busy ? "busy" : "idle" },
			pid: null,
			last_activity_at: busy ? now - 5000 : now - 12 * MIN,
		});
	const box = (s: AgentSeed): Agent =>
		seed(now, { workspace: APP_BOX, vendor: "codex", account: DEVBOX_CODEX, pid: 912, ...s });
	return [
		app({
			id: RUN_LEADS.analysis,
			title: "Search filters",
			activity: { state: "busy_in_subagents" },
		}),
		sub(RUN_LEADS.analysis, "agent-ex-testing", "Testing gate", true),
		sub(RUN_LEADS.analysis, "agent-ex-security", "Security gate", true),
		sub(RUN_LEADS.analysis, "agent-ex-a11y", "Accessibility gate", true),
		app({
			id: RUN_LEADS.approve,
			title: "Settings page redesign",
			activity: { state: "busy_in_subagents" },
		}),
		sub(RUN_LEADS.approve, "agent-ex-mockup", "Mockup round 2", true),
		box({
			id: RUN_LEADS.codex,
			title: "Export to CSV",
			activity: { state: "busy_in_subagents" },
		}),
		box({
			id: "agent-ex-codex-build",
			parent: RUN_LEADS.codex,
			title: "Implement the export",
			activity: { state: "busy" },
			pid: null,
		}),
		box({
			id: "agent-ex-codex-design",
			parent: RUN_LEADS.codex,
			title: "Design the export",
			activity: { state: "idle" },
			pid: null,
			last_activity_at: now - 40 * MIN,
		}),
		app({
			id: RUN_LEADS.loops,
			title: "Offline sync",
			activity: { state: "done_unread" },
			last_activity_at: now - 15 * MIN,
		}),
		seed(now, {
			id: "agent-ex-cursor-review",
			workspace: APP_CURSOR,
			vendor: "cursor",
			title: "Review offline sync",
			activity: { state: "idle" },
			last_activity_at: now - 30 * MIN,
		}),
		app({ id: RUN_LEADS.journal, title: "Date picker fix", activity: { state: "busy" } }),
		app({
			id: RUN_LEADS.agents,
			title: "Clean up old flags",
			activity: { state: "busy_in_subagents" },
		}),
		sub(RUN_LEADS.agents, "agent-ex-flags-web", "Flags in the web app", true),
		// "agent-ex-flags-docs" finished, so maxd no longer lists it.
		onMac(API_MAC, {
			id: RUN_LEADS.cr,
			title: "Rate limit design",
			activity: { state: "idle" },
			last_activity_at: now - 6 * MIN,
		}),
	];
};

// The pipeline every pipeline run shares: research, design with a mockup
// on a side lane for ui work, approval, implementation in chunks, five
// analysis gates in parallel, then verify, integrate, review and CI.
const GATES = ["testing", "security", "a11y", "chaos", "perf"] as const;

const shapeNode = (id: string, label: string, extra: Partial<NodeShape> = {}): NodeShape => ({
	id,
	label,
	group: null,
	lane: null,
	host: null,
	...extra,
});

const PIPELINE_NODES: NodeShape[] = [
	shapeNode("research", "Research"),
	shapeNode("design", "Design"),
	shapeNode("mockup", "Mockup", { lane: "side" }),
	shapeNode("approve", "Approve"),
	shapeNode("implement", "Implement"),
	shapeNode("testing", "Testing", { group: "analysis" }),
	shapeNode("security", "Security", { group: "analysis" }),
	shapeNode("a11y", "Accessibility", { group: "analysis" }),
	shapeNode("chaos", "Chaos", { group: "analysis" }),
	shapeNode("perf", "Performance", { group: "analysis" }),
	shapeNode("verify", "Verify"),
	shapeNode("integrate", "Integrate"),
	shapeNode("review", "Review"),
	shapeNode("ci", "CI"),
];

const edge = (from: string, to: string): EdgeShape => ({ from, to });

const PIPELINE_EDGES: EdgeShape[] = [
	edge("research", "design"),
	edge("design", "mockup"),
	edge("design", "approve"),
	edge("mockup", "approve"),
	edge("approve", "implement"),
	...GATES.map((gate) => edge("implement", gate)),
	...GATES.map((gate) => edge(gate, "verify")),
	edge("verify", "integrate"),
	edge("integrate", "review"),
	edge("review", "ci"),
];

const ANALYSIS: GroupShape = { id: "analysis", label: "Analysis", collapsed: true };

/** What a sample node sets; the rest is empty. */
export type NodeSeed = Partial<NodeState> & { readonly status: NodeStatus };

/** A node with only what `s` sets. */
export const nodeState = (id: string, s: NodeSeed): NodeState => ({
	id,
	provisional: false,
	note: null,
	badges: [],
	attempts: [],
	agents: [],
	quiet: false,
	links: [],
	...s,
});

const badge = (text: string, tone: Badge["tone"] = "neutral"): Badge => ({ text, tone });

/** One attempt from `start` minutes ago, ended `end` minutes ago unless it runs. */
const attempt = (
	now: Timestamp,
	start: number,
	end: number | null,
	agent: AgentId | null = null,
	endStatus: NodeStatus = "done"
): AttemptSpan => ({
	start: now - start * MIN,
	end: end === null ? null : now - end * MIN,
	end_status: end === null ? null : endStatus,
	agent,
});

/** One sample run: where it sits, what it looks like, and where it stands. */
type SampleRun = {
	readonly key: RunKey;
	readonly label: string;
	readonly orchestrators: AgentId[];
	readonly lastActivity: Timestamp;
	readonly nodes: NodeShape[];
	readonly edges: EdgeShape[];
	readonly groups: GroupShape[];
	readonly states: Record<string, NodeSeed>;
	readonly loops: LoopState[];
	readonly notice: RunNotice | null;
	readonly status: NodeStatus;
	/** The descriptor's header lines. */
	readonly lines?: Badge[];
};

const key = (device: string, project: string, workflow: string, run: string): RunKey => ({
	device,
	project,
	workflow,
	run,
});

const pipeline = (
	run: string,
	rest: Omit<SampleRun, "key" | "nodes" | "edges" | "groups"> & { readonly ui?: boolean }
): SampleRun => ({
	key: key(MAC, EXAMPLE_APP, "pipeline", run),
	nodes: PIPELINE_NODES,
	edges: PIPELINE_EDGES,
	groups: [ANALYSIS],
	...rest,
	// A run that isn't ui work leaves the mockup out.
	states: rest.ui ? rest.states : { mockup: { status: "omitted" }, ...rest.states },
});

const allDone = (now: Timestamp, from: number): Record<string, NodeSeed> =>
	Object.fromEntries(
		PIPELINE_NODES.map((node, i) => [
			node.id,
			{ status: "done", attempts: [attempt(now, from - i * 6, from - i * 6 - 5)] },
		])
	);

const samples = (now: Timestamp): SampleRun[] => [
	// 1. Busy in the analysis group, with three gates running as Claude sub-agents.
	pipeline("t-20261101-example-filters", {
		label: "Search filters",
		orchestrators: [RUN_LEADS.analysis],
		lastActivity: now - 5000,
		status: "busy",
		states: {
			research: { status: "done", attempts: [attempt(now, 110, 96)] },
			design: { status: "done", attempts: [attempt(now, 95, 80)] },
			approve: { status: "skipped", note: "Not ui work, so nothing to approve" },
			implement: {
				status: "done",
				badges: [badge("chunk 4/4")],
				attempts: [attempt(now, 79, 14)],
			},
			testing: {
				status: "busy",
				agents: ["agent-ex-testing"],
				attempts: [attempt(now, 13, null, "agent-ex-testing")],
			},
			security: {
				status: "busy",
				agents: ["agent-ex-security"],
				attempts: [attempt(now, 13, null, "agent-ex-security")],
			},
			a11y: {
				status: "busy",
				agents: ["agent-ex-a11y"],
				attempts: [attempt(now, 12, null, "agent-ex-a11y")],
			},
			chaos: { status: "skipped", note: "Effort is low" },
			perf: { status: "skipped", note: "Effort is low" },
			verify: { status: "queued" },
			integrate: { status: "queued" },
			review: { status: "queued" },
			ci: { status: "queued" },
		},
		loops: [],
		notice: null,
	}),
	// 2. Waiting on you: approval round 2 with the mockup back in progress.
	pipeline("t-20261101-example-settings", {
		label: "Settings page redesign",
		ui: true,
		orchestrators: [RUN_LEADS.approve],
		lastActivity: now - 2 * MIN,
		status: "waiting_human",
		states: {
			research: { status: "done", attempts: [attempt(now, 90, 75)] },
			design: { status: "done", attempts: [attempt(now, 74, 60)] },
			mockup: {
				status: "busy",
				badges: [badge("round 2")],
				agents: ["agent-ex-mockup"],
				attempts: [
					attempt(now, 59, 40, null, "done"),
					attempt(now, 8, null, "agent-ex-mockup"),
				],
			},
			approve: {
				status: "waiting_human",
				badges: [badge("round 2", "warn"), badge("3 notes")],
				attempts: [
					attempt(now, 39, 9, null, "failed"),
					{ ...attempt(now, 2, null), waiting_since: now - MIN },
				],
				links: [{ label: "Open review frame", url: "http://localhost:49160/" }],
			},
			implement: { status: "queued" },
			testing: { status: "queued" },
			security: { status: "queued" },
			a11y: { status: "queued" },
			chaos: { status: "will_skip", provisional: true },
			perf: { status: "will_skip", provisional: true },
			verify: { status: "queued" },
			integrate: { status: "queued" },
			review: { status: "queued" },
			ci: { status: "queued" },
		},
		loops: [
			{ from: "approve", to: "mockup", label: "approve", count: 1, budget: 3 },
			// A loop the workflow allows that hasn't run yet.
			{ from: "review", to: "implement", label: "review", count: 0, budget: 3 },
		],
		notice: null,
	}),
	// 3. Codex on devbox, its sub-agents bound by their prompts, with one node stale.
	{
		key: key(DEVBOX, EXAMPLE_APP, "pipeline", "t-20261101-example-export"),
		label: "Export to CSV",
		orchestrators: [RUN_LEADS.codex],
		lastActivity: now - 3 * MIN,
		status: "busy",
		nodes: PIPELINE_NODES,
		edges: PIPELINE_EDGES,
		groups: [ANALYSIS],
		states: {
			research: { status: "done", attempts: [attempt(now, 140, 120)] },
			design: {
				status: "stale",
				note: "The spec changed after this finished",
				agents: ["agent-ex-codex-design"],
				attempts: [attempt(now, 119, 70, "agent-ex-codex-design")],
			},
			mockup: { status: "omitted" },
			approve: { status: "skipped" },
			implement: {
				status: "busy",
				badges: [badge("chunk 2/3")],
				agents: ["agent-ex-codex-build"],
				attempts: [attempt(now, 69, null, "agent-ex-codex-build")],
			},
			...Object.fromEntries(GATES.map((gate) => [gate, { status: "queued" } as NodeSeed])),
			verify: { status: "queued" },
			integrate: { status: "queued" },
			review: { status: "queued" },
			ci: { status: "queued" },
		},
		loops: [],
		notice: null,
	},
	// 4. Finished, after loops: integrate twice of three, a re-verify and a red CI.
	pipeline("t-20261101-example-sync", {
		label: "Offline sync",
		orchestrators: [RUN_LEADS.loops],
		lastActivity: now - 15 * MIN,
		status: "done",
		states: {
			...allDone(now, 300),
			mockup: { status: "omitted" },
			implement: {
				status: "done",
				attempts: [
					attempt(now, 270, 200, null, "done"),
					attempt(now, 150, 120, null, "done"),
					attempt(now, 80, 60, null, "done"),
				],
			},
			review: {
				status: "done",
				agents: ["agent-ex-cursor-review"],
				attempts: [attempt(now, 40, 30, "agent-ex-cursor-review")],
			},
		},
		loops: [
			{ from: "integrate", to: "implement", label: "integrate", count: 2, budget: 3 },
			{ from: "verify", to: "implement", label: "re-verify", count: 1, budget: null },
			{ from: "ci", to: "implement", label: "CI red", count: 1, budget: null },
		],
		notice: null,
		lines: [badge("retries 10/12", "warn")],
	}),
	// 5. No event log, so its times come from the journal's lines.
	pipeline("t-20261101-example-dates", {
		label: "Date picker fix",
		orchestrators: [RUN_LEADS.journal],
		lastActivity: now - 60_000,
		status: "busy",
		states: {
			research: { status: "done", attempts: [attempt(now, 50, 41)] },
			design: { status: "done", attempts: [attempt(now, 40, 30)] },
			approve: { status: "skipped" },
			implement: { status: "busy", provisional: true, attempts: [attempt(now, 29, null)] },
			...Object.fromEntries(GATES.map((gate) => [gate, { status: "queued" } as NodeSeed])),
			verify: { status: "queued" },
			integrate: { status: "queued" },
			review: { status: "queued" },
			ci: { status: "queued" },
		},
		loops: [],
		notice: {
			text: "This run has no event log, so its times come from the journal",
			tone: "info",
		},
	}),
	// 6. No descriptor: the lead's sub-agents as its nodes.
	{
		key: key(MAC, EXAMPLE_APP, "maxhq.agents", RUN_LEADS.agents),
		label: "Clean up old flags",
		orchestrators: [RUN_LEADS.agents],
		lastActivity: now - 5000,
		status: "busy",
		nodes: [
			shapeNode("agent-ex-flags-web", "Flags in the web app"),
			shapeNode("agent-ex-flags-docs", "Flags in the docs"),
		],
		edges: [],
		groups: [],
		states: {
			"agent-ex-flags-web": {
				status: "busy",
				agents: ["agent-ex-flags-web"],
				attempts: [attempt(now, 20, null, "agent-ex-flags-web")],
			},
			"agent-ex-flags-docs": {
				status: "done",
				agents: ["agent-ex-flags-docs"],
				attempts: [attempt(now, 20, 12, "agent-ex-flags-docs")],
			},
		},
		loops: [],
		notice: null,
	},
	// 7. No agent leads it yet.
	{
		key: key(MAC, EXAMPLE_API, "pipeline", "t-20261101-example-webhooks"),
		label: "Webhook retries",
		orchestrators: [],
		lastActivity: now - 9 * MIN,
		status: "busy",
		nodes: PIPELINE_NODES,
		edges: PIPELINE_EDGES,
		groups: [ANALYSIS],
		states: {
			research: { status: "done", attempts: [attempt(now, 30, 18)] },
			design: { status: "busy", attempts: [attempt(now, 17, null)] },
			mockup: { status: "omitted" },
			approve: { status: "queued" },
			implement: { status: "queued" },
			...Object.fromEntries(GATES.map((gate) => [gate, { status: "queued" } as NodeSeed])),
			verify: { status: "queued" },
			integrate: { status: "queued" },
			review: { status: "queued" },
			ci: { status: "queued" },
		},
		loops: [],
		notice: null,
	},
	// 8. A cr run at its round limit, waiting on you.
	{
		key: key(MAC, EXAMPLE_API, "cr", "t-20261101-example-limits"),
		label: "Rate limit design",
		orchestrators: [RUN_LEADS.cr],
		lastActivity: now - 6 * MIN,
		status: "waiting_human",
		nodes: [
			shapeNode("cr-plan", "Plan"),
			shapeNode("cr-design", "Design"),
			shapeNode("cr-build", "Build"),
		],
		edges: [edge("cr-plan", "cr-design"), edge("cr-design", "cr-build")],
		groups: [],
		states: {
			"cr-plan": {
				status: "done",
				badges: [badge("round 1/3")],
				attempts: [attempt(now, 60, 45)],
			},
			"cr-design": {
				status: "waiting_human",
				note: "Hit the round limit",
				badges: [badge("round 3/3", "bad")],
				attempts: [attempt(now, 44, 30), attempt(now, 29, 18), attempt(now, 17, 6)],
			},
			"cr-build": { status: "queued" },
		},
		loops: [{ from: "cr-design", to: "cr-design", label: "round", count: 3, budget: 3 }],
		notice: null,
	},
];

const stateOf = (sample: SampleRun, node: NodeShape): NodeState =>
	nodeState(node.id, sample.states[node.id] ?? { status: "queued" });

const progressOf = (states: NodeState[]) => {
	const counted = states.filter((state) => state.status !== "omitted");
	const done = counted.filter((state) => state.status === "done" || state.status === "skipped");
	return { done: done.length, total: counted.length };
};

const snapshotOf = (sample: SampleRun): RunSnapshot => {
	const nodes = sample.nodes.map((node) => stateOf(sample, node));
	return {
		key: sample.key,
		rev: 1,
		shape: {
			nodes: sample.nodes,
			edges: sample.edges,
			groups: sample.groups,
			hash: `${sample.key.workflow}-${sample.nodes.length}`,
		},
		nodes,
		loops: sample.loops,
		header: {
			label: sample.label,
			status: sample.status,
			progress: progressOf(nodes),
			lines: sample.lines ?? [],
		},
		notice: sample.notice,
	};
};

const entityOf = (sample: SampleRun): WorkflowRun => {
	const nodes = sample.nodes.map((node) => stateOf(sample, node));
	return {
		id: `wfr-${sample.key.run}`,
		key: sample.key,
		label: sample.label,
		progress: progressOf(nodes),
		current: nodes.filter((node) => node.status === "busy").map((node) => node.id),
		needs_you: nodes.some((node) => node.status === "waiting_human"),
		orchestrators: sample.orchestrators,
		last_activity: sample.lastActivity,
		done: sample.status === "done",
		agents: [...new Set(nodes.flatMap((node) => node.agents))].filter(
			(agent) => !sample.orchestrators.includes(agent)
		),
	};
};

/**
 * The sample workflow runs, with the projects, workspaces and agents they
 * sit in, each as its own entity the way `maxd` publishes them.
 */
export const mockWorkflowRuns = (now: Timestamp): Entity[] => [
	...[project(EXAMPLE_APP, "example-app"), project(EXAMPLE_API, "example-api")].map((v) => ({
		entity: "project" as const,
		...v,
	})),
	...workspaces().map((v) => ({ entity: "workspace" as const, ...v })),
	...agents(now).map((v) => ({ entity: "agent" as const, ...v })),
	...samples(now).map((sample) => ({ entity: "workflow_run" as const, ...entityOf(sample) })),
];

/** The snapshot each sample run opens with, by `runName`. */
export const mockRunSnapshots = (now: Timestamp): Map<string, RunSnapshot> =>
	new Map(samples(now).map((sample) => [runName(sample.key), snapshotOf(sample)]));

const sampleOf = (now: Timestamp, run: string): SampleRun => {
	const sample = samples(now).find((each) => each.key.run === run);
	if (!sample) throw new Error(`No sample run ${run}`);
	return sample;
};

/** One sample run's entity, by its own id, such as "t-20261101-example-sync". */
export const mockRunEntity = (now: Timestamp, run: string): WorkflowRun =>
	entityOf(sampleOf(now, run));

/** One sample run's snapshot, by its own id. */
export const mockRunSnapshot = (now: Timestamp, run: string): RunSnapshot =>
	snapshotOf(sampleOf(now, run));

/** What opening a sample node shows. */
export const mockNodeDetail = (snapshot: RunSnapshot, node: string): DetailList[] => {
	const state = snapshot.nodes.find((each) => each.id === node);
	if (!state) return [];
	const lists: DetailList[] = [
		{ label: "Files read", items: ["src/search/filters.ts", "docs/spec.md"] },
		{ label: "Files written", items: state.status === "done" ? ["src/search/filters.ts"] : [] },
	];
	if (state.note) lists.push({ label: "Notes", items: [state.note] });
	return lists;
};
