// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The sample dispatcher on the Mac. A request starts a conversation that
// asks which project you mean, then proposes a plan for it: a Cursor agent
// there, and a task that sends it octocat's PR changes once 15 minutes pass
// with none. A confirmed plan runs through the same sample calls as the
// rest of the source, item by item, and stops at the first that fails.

import type {
	ActionOutcome,
	AgentSpec,
	AttentionAnswer,
	AttentionId,
	Dispatcher,
	PlanItem,
	PlanOutcome,
	PlanStep,
	Plan,
	Task,
	TaskTarget,
	Timestamp,
} from "../bindings";
import type { Entity } from "../fleet/wire";
import { NO_PERMISSION } from "../tasks/draft";

import { NO_RULES } from "./mock-tasks";
import { type MockTimelines, say, you } from "./mock-timeline";
import { CURSOR, MAC } from "./mock-view";
import type { ActionResult, DataSource } from "./source";

/** The workspace of the dispatcher's own folder on the Mac. */
export const DISPATCHER_HOME = "ws-dispatcher-mac";

/** The sample GitHub login the plan watches. */
export const SAMPLE_AUTHOR = "octocat";

/** The dispatcher's folder and its entity, as the Mac's `maxd` publishes them. */
export const dispatcherEntities = (): Entity[] => [
	{
		entity: "workspace",
		id: DISPATCHER_HOME,
		project: null,
		device: MAC,
		location: {
			type: "folder",
			path: "/Users/me/Library/Application Support/MaxHQ/dispatcher",
		},
	},
	{ entity: "dispatcher", device: MAC, workspace: DISPATCHER_HOME, agent: null, plan: null },
];

/** The dispatcher calls of a data source. */
export type DispatcherCalls = Pick<DataSource, "askDispatcher" | "confirmPlan" | "dismissPlan">;

/** The sample dispatcher. */
export type MockDispatcher = {
	readonly calls: DispatcherCalls;
	/** Takes an answer to the dispatcher's question. Null when it's another agent's. */
	readonly answer: (event: AttentionId, answer: AttentionAnswer) => ActionResult | null;
};

/** What the sample dispatcher needs from the rest of the source. */
export type DispatcherContext = {
	readonly now: () => Timestamp;
	/** An entity as the source holds it now, by "kind:id". */
	readonly get: (key: string) => Entity | undefined;
	/** Every entity the source holds now. */
	readonly entities: () => Iterable<Entity>;
	/** Changes the sample entities and publishes the change. */
	readonly change: (edit: (base: Entity[]) => Entity[]) => void;
	readonly timelines: Pick<MockTimelines, "post" | "decide">;
	readonly startAgent: DataSource["startAgent"];
	readonly saveTask: DataSource["saveTask"];
};

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

const ran = (results: PlanOutcome[]): ActionResult => ({
	status: "ok",
	data: { status: "plan_ran", results },
});

const QUESTION = "Which project should the agent work in?";

/** A project with a folder on the Mac, which the plan's agent can run in. */
type Choice = {
	readonly name: string;
	readonly workspace: string;
	readonly path: string;
	readonly repo: string;
};

/** Why a call didn't give what the plan needed, for a person to read. */
const why = (result: ActionResult, fallback: string): string => {
	if (result.status === "error") return fallback;
	const data: ActionOutcome = result.data;
	return data.status === "refused" ? data.reason : fallback;
};

/** The sample dispatcher, over the source's entities and calls. */
export const mockDispatcher = (ctx: DispatcherContext): MockDispatcher => {
	let asked = 0;
	let plans = 0;
	// The question waiting for an answer, and the project the last plan was for.
	let open: { readonly attention: AttentionId; readonly agent: string } | null = null;
	let chosen: Choice | null = null;

	const state = (): Dispatcher | undefined => {
		const found = ctx.get(`dispatcher:${MAC}`);
		return found?.entity === "dispatcher" ? found : undefined;
	};

	const setState = (edit: (d: Dispatcher) => Dispatcher) =>
		ctx.change((base) =>
			base.map((e) =>
				e.entity === "dispatcher" && e.device === MAC ? { ...e, ...edit(e) } : e
			)
		);

	const choices = (): Choice[] => {
		const out: Choice[] = [];
		for (const e of ctx.entities()) {
			if (e.entity !== "workspace" || e.device !== MAC || !e.project) continue;
			if (e.location.type !== "folder") continue;
			const project = ctx.get(`project:${e.project}`);
			if (project?.entity !== "project") continue;
			out.push({
				name: project.name,
				workspace: e.id,
				path: e.location.path,
				repo: (project.repo ?? "").replace(/^github\.com\//, ""),
			});
		}
		return out.sort((a, b) => a.name.localeCompare(b.name));
	};

	const ask = (agent: string) => {
		asked += 1;
		const attention = `att-dispatcher-${asked}`;
		const options = choices();
		open = { attention, agent };
		ctx.timelines.post(
			agent,
			{
				kind: "question",
				attention,
				questions: [
					{
						id: "project",
						header: "Project",
						text: QUESTION,
						options: options.map((c) => ({ label: c.name, description: c.path })),
						multi: false,
						other: false,
						secret: false,
					},
				],
				answers: null,
			},
			"waiting"
		);
		const at = ctx.now();
		ctx.change((base) => [
			...base.map((e) =>
				e.entity === "agent" && e.id === agent
					? {
							...e,
							activity: { state: "needs_input" as const, attention },
							last_activity_at: at,
						}
					: e
			),
			{
				entity: "attention",
				id: attention,
				agent,
				kind: "question",
				summary: QUESTION,
				options: options.map((c) => ({ id: c.name, label: c.name })),
				accepts_text: false,
				created_at: at,
				expires_at: null,
			},
		]);
	};

	const items = (project: Choice): PlanItem[] => {
		const spec: AgentSpec = {
			vendor: "cursor",
			account: CURSOR,
			place: { type: "workspace", workspace: project.workspace },
			model: null,
			permission: null,
			title: "PR reviews",
			first: null,
			branch: null,
		};
		return [
			{
				reason: `Reviews ${SAMPLE_AUTHOR}'s PRs`,
				step: { type: "start_agent", device: MAC, spec },
			},
			{
				reason: `Sends it each batch of ${SAMPLE_AUTHOR}'s PR changes`,
				step: {
					type: "schedule_task",
					device: MAC,
					name: `Review ${SAMPLE_AUTHOR}'s PRs`,
					prompt: "Review the PR changes in this batch.",
					trigger: {
						type: "github_events",
						repo: project.repo,
						events: ["pull_request"],
						rules: {
							...NO_RULES,
							filters: [
								{
									path: "body.pull_request.user.login",
									test: { op: "equals", value: SAMPLE_AUTHOR },
								},
							],
							coalesce_key: SAMPLE_AUTHOR,
							coalesce_secs: 15 * 60,
							coalesce_restart: true,
						},
					},
					target: { type: "item", item: 0 },
				},
			},
		];
	};

	const propose = (agent: string, project: Choice) => {
		chosen = project;
		plans += 1;
		const plan: Plan = { id: plans, items: items(project) };
		ctx.timelines.post(
			agent,
			say(`Here's the plan for ${project.name}. Nothing starts until you confirm it.`)
		);
		setState((d) => ({ ...d, plan }));
	};

	/** Runs one item, with the results of the items before it. */
	const run = async (
		step: PlanStep,
		before: readonly PlanOutcome[],
		taskId: string
	): Promise<PlanOutcome> => {
		switch (step.type) {
			case "start_agent": {
				const result = await ctx.startAgent(step.spec);
				return result.status === "ok" && result.data.status === "started"
					? { type: "started", agent: result.data.agent }
					: { type: "failed", reason: why(result, "The agent didn't start.") };
			}
			case "send_message":
				return ctx.get(`agent:${step.agent}`)
					? { type: "sent", agent: step.agent }
					: {
							type: "failed",
							reason: `There's no agent on this Mac with id ${step.agent}.`,
						};
			case "schedule_task": {
				let target: TaskTarget;
				if (step.target.type === "item") {
					const { item } = step.target;
					const started = before[item];
					if (started?.type !== "started")
						return { type: "failed", reason: `Item ${item} didn't start an agent.` };
					target = { type: "agent", device: step.device, agent: started.agent };
				} else target = step.target.target;
				const task: Task = {
					id: taskId,
					name: step.name,
					prompt: step.prompt,
					trigger: step.trigger,
					target,
					enabled: true,
					permission: NO_PERMISSION,
					made_by: { type: "user" },
					next_run: null,
					last_run: null,
				};
				const result = await ctx.saveTask(step.device, task);
				return result.status === "ok" && result.data.status === "task_saved"
					? { type: "task_saved", task: result.data.task.id }
					: { type: "failed", reason: why(result, "The task wasn't saved.") };
			}
			case "move_agent":
				return { type: "failed", reason: "The sample app doesn't move agents." };
		}
	};

	const calls: DispatcherCalls = {
		askDispatcher: async (device, text, account, fresh) => {
			const dispatcher = state();
			if (device !== MAC || !dispatcher) return refused("The dispatcher runs on the Mac.");
			if (!text.trim()) return refused("Type a request first.");
			const login = ctx.get(`account:${account}`);
			if (
				login?.entity !== "account" ||
				login.device !== MAC ||
				(login.vendor !== "claude" && login.vendor !== "codex")
			)
				return refused("The dispatcher runs as a Claude or Codex account on this Mac.");
			const held = dispatcher.agent ? ctx.get(`agent:${dispatcher.agent}`) : undefined;
			if (!fresh && held?.entity === "agent" && held.account === account) {
				ctx.timelines.post(held.id, you(text));
				// A follow-up with no question waiting gets the plan again, as a new one.
				if (!open && chosen) propose(held.id, chosen);
				return DONE;
			}
			// A new conversation stops the old one and drops its plan.
			const old = dispatcher.agent;
			if (old) {
				open = null;
				ctx.change((base) =>
					base.filter(
						(e) =>
							!(e.entity === "agent" && e.id === old) &&
							!(e.entity === "attention" && e.agent === old)
					)
				);
			}
			setState((d) => ({ ...d, agent: null, plan: null }));
			const started = await ctx.startAgent({
				vendor: login.vendor,
				account,
				place: { type: "workspace", workspace: dispatcher.workspace },
				model: null,
				permission: "ask",
				title: "Dispatcher",
				first: null,
				branch: null,
			});
			if (started.status !== "ok" || started.data.status !== "started") return started;
			const agent = started.data.agent;
			setState((d) => ({ ...d, agent }));
			ctx.timelines.post(agent, you(text));
			ask(agent);
			return started;
		},
		confirmPlan: async (device, id) => {
			const plan = state()?.plan;
			if (device !== MAC) return refused("The dispatcher runs on the Mac.");
			if (!plan || plan.id !== id) return refused("That plan isn't the one shown any more.");
			setState((d) => ({ ...d, plan: null }));
			const results: PlanOutcome[] = [];
			for (const [i, item] of plan.items.entries()) {
				const outcome = await run(item.step, results, `task-dispatch-${plan.id}-${i}`);
				results.push(outcome);
				if (outcome.type === "failed") break;
			}
			return ran(results);
		},
		dismissPlan: async (device, id) => {
			const plan = state()?.plan;
			if (device !== MAC) return refused("The dispatcher runs on the Mac.");
			if (!plan || plan.id !== id) return refused("That plan isn't the one shown any more.");
			setState((d) => ({ ...d, plan: null }));
			return DONE;
		},
	};

	return {
		calls,
		answer: (event, answer) => {
			if (!open || open.attention !== event) return null;
			const { agent } = open;
			open = null;
			ctx.timelines.decide(event, answer, null);
			const picked =
				answer.type === "questions"
					? answer.answers[0]?.values[0]
					: answer.type === "text"
						? answer.text
						: answer.type === "option"
							? answer.id
							: null;
			const options = choices();
			// Dismissed, the dispatcher proposes nothing.
			const project =
				answer.type === "dismiss"
					? undefined
					: (options.find((c) => c.name === picked) ?? options[0]);
			ctx.change((base) =>
				base
					.filter((e) => !(e.entity === "attention" && e.id === event))
					.map((e) =>
						e.entity === "agent" && e.id === agent
							? { ...e, activity: { state: "idle" as const } }
							: e
					)
			);
			if (project) propose(agent, project);
			else if (answer.type !== "dismiss")
				ctx.timelines.post(agent, say("There's no project with a folder on this Mac yet."));
			return DONE;
		},
	};
};
