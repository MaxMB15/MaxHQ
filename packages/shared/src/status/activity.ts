// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The activity dot from decision N-1, and what its hover and the sidebar row
// say for each state. Claude reports when its cache goes cold. Codex doesn't,
// so a warm Codex agent is an estimate and its hover says so (C-7).

import { t } from "../i18n";

import type { Account, Activity, Agent, AttentionEvent, PauseReason, Timestamp } from "../bindings";
import type { ViewState } from "../view/replica";
import { agentLinkContext, childrenOf } from "../view/select";
import { commandWaits } from "../terminal/select";

import type { Fact, HoverCard } from "./card";
import { agentConnection, type ConnectionLook } from "./connection";
import {
	askTitle,
	formatAgo,
	formatAgoShort,
	formatContext,
	formatDuration,
	formatWhen,
	listInWords,
	plural,
	sentence,
	showHidden,
} from "./format";
import { agentWait, idleExtra, jobsFact, jobsSentence, waitWords } from "./running";
import { agentKind, isLocalAccount, kindName } from "./agent-kind";
import { accountName, accountSummary, blockingWindow } from "./usage";

/** How the activity dot looks. One per state, with a warm cache past its time shown as idle. */
export type ActivityLook =
	| "starting"
	| "busy"
	| "busy_in_subagents"
	| "needs_input"
	| "done_unread"
	| "idle_cache_warm"
	| "idle"
	| "idle_running"
	| "idle_waiting"
	| "waiting_for_limit_reset"
	| "paused"
	| "error"
	| "not_accessible";

/** The look for an activity at a moment. */
export const activityLook = (activity: Activity, now: Timestamp): ActivityLook =>
	activity.state === "idle_cache_warm" && activity.warm_until <= now ? "idle" : activity.state;

/**
 * The look for an agent at a moment: its activity's, except that an idle
 * agent that waits on something, or left processes running, says so. It
 * never needs you, so a turn you haven't read keeps its own look.
 */
export const agentLook = (
	agent: Pick<Agent, "activity" | "running">,
	now: Timestamp
): ActivityLook => {
	const look = activityLook(agent.activity, now);
	return look === "idle" || look === "idle_cache_warm" ? (idleExtra(agent, now) ?? look) : look;
};

/** The words a screen reader hears for each look. */
export const ACTIVITY_LABEL: Record<ActivityLook, string> = {
	get starting() {
		return t("Starting");
	},
	get busy() {
		return t("Busy");
	},
	get busy_in_subagents() {
		return t("Sub-agents busy");
	},
	get needs_input() {
		return t("Needs input");
	},
	get done_unread() {
		return t("Done, unread");
	},
	get idle_cache_warm() {
		return t("Idle, cache warm");
	},
	get idle() {
		return t("Idle");
	},
	get idle_running() {
		return t("Done, processes still running");
	},
	get idle_waiting() {
		return t("Waiting on background work");
	},
	get waiting_for_limit_reset() {
		return t("Waiting for a limit");
	},
	get paused() {
		return t("Paused");
	},
	get error() {
		return t("Error");
	},
	get not_accessible() {
		return t("Not accessible");
	},
};

/** Whether the agent is doing or waiting on work, so it never folds away. */
export const isWorking = (activity: Activity): boolean =>
	activity.state === "busy" ||
	activity.state === "busy_in_subagents" ||
	activity.state === "starting";

/** Why an agent stopped with an error, as a sentence such as "Exited with status 1.". */
export const errorReason = (reason: string): string => `${sentence(reason)}.`;

/** How long after its start a starting agent counts as resuming rather than launching. */
const RESUME_AFTER_MS = 2 * 60 * 1000;

/** Whether a starting agent is coming back from a restart rather than launching. */
export const isResuming = (
	agent: Pick<Agent, "activity" | "started_at">,
	now: Timestamp
): boolean => agent.activity.state === "starting" && now - agent.started_at > RESUME_AFTER_MS;

/** What the hover needs beyond the agent itself. */
export type ActivityContext = {
	/** The agent the dot is for. */
	readonly agent: Agent;
	/** The login it runs under, if the view has it. */
	readonly account?: Account;
	/** What a local agent goes by, such as "Local · Qwen3.6 35B A3B", in place of its account. */
	readonly localLabel?: string;
	/** The open prompt, while it needs input. */
	readonly attention?: AttentionEvent;
	/** Its sub-agents. */
	readonly children?: readonly Agent[];
	/** The moment to describe. */
	readonly now: Timestamp;
};

const facts = (ctx: ActivityContext, warm?: string): Fact[] => {
	const out: Fact[] = [];
	const context = ctx.agent.context;
	if (context) {
		const value = formatContext(context.used_tokens, context.window_tokens);
		out.push({ label: t("Context"), value: warm ? `${value}, ${warm}` : value });
	}
	if (ctx.account && isLocalAccount(ctx.account))
		out.push({ label: t("Agent"), value: ctx.localLabel ?? accountName(ctx.account) });
	else if (ctx.account) out.push({ label: t("Account"), value: accountSummary(ctx.account) });
	return out;
};

/** The facts for an agent done with its turn, with the jobs it left running. */
const doneFacts = (ctx: ActivityContext, warm?: string): Fact[] => [
	...jobsFact(ctx.agent.running),
	...facts(ctx, warm),
];

/** The card for an idle agent that waits on something or left processes running. */
const runningCard = (ctx: ActivityContext): HoverCard | null => {
	const { agent, now } = ctx;
	const wait = agentWait(agent.running, now);
	if (wait) {
		const { title, body } = waitWords(wait, now);
		return { title, body, facts: doneFacts(ctx), foot: lastActivity(ctx) };
	}
	const running = agent.running;
	if (!running || running.count === 0) return null;
	return {
		title: t("Done, processes still running"),
		body: jobsSentence(running),
		facts: facts(ctx),
		foot: lastActivity(ctx),
	};
};

const lastActivity = (ctx: ActivityContext) =>
	t("Last activity {{formatAgo_ctx_agent_last_activity_at__ctx_now}}", {
		formatAgo_ctx_agent_last_activity_at__ctx_now: formatAgo(
			ctx.agent.last_activity_at,
			ctx.now
		),
	});

const working = (children: readonly Agent[]) =>
	children.filter((c) => isWorking(c.activity) || c.activity.state === "needs_input");

/**
 * The hover card for a paused agent. Stop pauses an agent, and Resume or a
 * message you send ends any pause.
 */
const pausedCard = (reason: PauseReason, ctx: ActivityContext, foot: string): HoverCard => {
	switch (reason) {
		case "by_you":
			return {
				title: t("Paused"),
				body: t(
					"You stopped it. Resume starts it again, and so does sending it a message."
				),
				facts: facts(ctx),
				foot,
			};
		case "resume_off":
			return {
				title: t("Paused after a restart"),
				body: t(
					"Its device or container restarted, and Resume after a restart is off for its project. Resume starts it again, and so does sending it a message."
				),
				facts: facts(ctx),
				foot,
			};
		case "resumes_made_no_progress":
			return {
				title: t("Paused after 3 resumes"),
				body: t(
					"It was resumed 3 times in a row without making progress, so it waits for you to look."
				),
				facts: facts(ctx),
				foot: t(
					"A resume counts toward 3 only if the agent made no progress since the last one"
				),
			};
	}
};

/** The hover card for the activity dot. */
export const activityCard = (ctx: ActivityContext): HoverCard => {
	const { agent, now } = ctx;
	const activity = agent.activity;
	const children = ctx.children ?? [];
	const foot = lastActivity(ctx);
	const look = activityLook(activity, now);
	if (look === "idle" || look === "idle_cache_warm") {
		const card = runningCard(ctx);
		if (card) return card;
	}

	switch (activity.state) {
		case "busy": {
			const running = working(children);
			const body =
				running.length > 0
					? t("Sub-agents running", {
							count: running.length,
							names: listInWords(running.map((c) => c.title)),
						})
					: undefined;
			return { title: t("Busy"), body, facts: facts(ctx), foot };
		}
		case "busy_in_subagents": {
			const running = working(children);
			return {
				title:
					running.length > 0
						? t("Waiting on {{plural_running_length___sub_agent}}", {
								plural_running_length___sub_agent: plural(
									running.length,
									"sub-agent"
								),
							})
						: t("Waiting on sub-agents"),
				body:
					running.length > 0
						? t("Sub-agents still working", {
								count: running.length,
								names: listInWords(running.map((c) => c.title)),
							})
						: t("Its sub-agents are still working. The lead goes on once they finish."),
				facts: facts(ctx),
				foot,
			};
		}
		case "starting":
			return isResuming(agent, now)
				? {
						title: t("Resuming after a restart"),
						body: t(
							"MaxHQ reopened the session, and the agent picks up where it stopped."
						),
						facts: facts(ctx),
						foot: t("First started {{formatAgo_agent_started_at__now}}", {
							formatAgo_agent_started_at__now: formatAgo(agent.started_at, now),
						}),
					}
				: {
						title: t("Starting"),
						body: t("The CLI is launching."),
						facts: facts(ctx),
						foot,
					};
		case "needs_input": {
			const attention = ctx.attention;
			const title =
				attention?.kind === "permission"
					? t("Asking for permission")
					: attention?.kind === "question"
						? t("Asking you a question")
						: t("Needs input");
			const answer =
				agent.control === "observed"
					? t("Answer it in the terminal you started it from")
					: t("Answer it in its pane");
			return {
				title,
				body: attention ? showHidden(askTitle(attention.summary)) : undefined,
				facts: facts(ctx),
				foot: attention
					? t("Asked {{formatAgo_attention_created_at__now}}. {{answer}}", {
							formatAgo_attention_created_at__now: formatAgo(
								attention.created_at,
								now
							),
							answer: answer,
						})
					: answer,
			};
		}
		case "done_unread":
			return {
				title: t("Done, not opened yet"),
				body: t(
					"Finished a turn {{formatAgo_agent_last_activity_at__now}}. Open it to read the result.",
					{
						formatAgo_agent_last_activity_at__now: formatAgo(
							agent.last_activity_at,
							now
						),
					}
				),
				facts: doneFacts(ctx),
			};
		case "idle_cache_warm": {
			const left = activity.warm_until - now;
			if (left <= 0) return idleCard(ctx);
			// Only Claude reports its cache; a local model's server doesn't.
			const kind = agentKind(agent, ctx.account ? [ctx.account] : []);
			const estimate = kind !== "claude";
			const until = t("{{value1}}, in {{value2}}", {
				value1: formatWhen(activity.warm_until, now),
				value2: formatDuration(left),
			});
			return {
				title: t("Idle, cache warm"),
				body: estimate
					? t(
							"The cache is probably warm until {{until}}. {{VENDOR_NAME_agent_vendor}} doesn't report its cache, so this is an estimate from when the last turn finished.",
							{
								until: until,
								VENDOR_NAME_agent_vendor: kindName(kind),
							}
						)
					: t(
							"The cache stays warm until {{until}}, so the next message is cheap and fast.",
							{ until: until }
						),
				facts: facts(ctx, estimate ? t("cache warm, estimate") : t("cache warm")),
				foot,
			};
		}
		case "idle":
			return idleCard(ctx);
		case "waiting_for_limit_reset": {
			const name = ctx.account ? accountName(ctx.account) : undefined;
			const window = ctx.account ? blockingWindow(ctx.account) : undefined;
			const resetsAt = activity.resets_at ?? window?.resetsAt ?? null;
			const which =
				window?.blockedByWeek || (window && window.minutes >= 7 * 24 * 60)
					? t("weekly")
					: t("5-hour");
			const when =
				resetsAt !== null
					? t(
							"The {{which}} limit resets at {{formatWhen_resetsAt__now}}, in {{formatDuration_Math_max_0__resetsAt___now}}.",
							{
								which: which,
								formatWhen_resetsAt__now: formatWhen(resetsAt, now),
								formatDuration_Math_max_0__resetsAt___now: formatDuration(
									Math.max(0, resetsAt - now)
								),
							}
						)
					: t("The vendor didn't say when the limit resets.");
			return {
				title: name
					? t("Waiting for {{name}}", { name: name })
					: t("Waiting for a usage limit"),
				body: t("{{when}} The agent sends its next turn then, with nothing lost.", {
					when: when,
				}),
				facts: facts(ctx),
			};
		}
		case "paused":
			return pausedCard(activity.reason, ctx, foot);
		case "error":
			return {
				title: t("Stopped with an error"),
				body: errorReason(activity.reason),
				facts: facts(ctx),
				foot,
			};
		case "not_accessible":
			return {
				title: t("Not accessible"),
				body: t("MaxHQ can't see this agent right now. The connection dot says why."),
				facts: facts(ctx),
				foot,
			};
	}
};

const idleCard = (ctx: ActivityContext): HoverCard => ({
	title: t("Idle"),
	body: t("Waiting for a task. The next message re-reads its context."),
	facts: facts(ctx),
	foot: lastActivity(ctx),
});

/** What sits at the end of a sidebar row, before the connection dot. */
export type RowMeta =
	/** The green "Input" pill. */
	| { readonly kind: "input" }
	/** A few words or a time, such as "4m", "3 sub", "17:10" or "Resuming". */
	| { readonly kind: "text"; readonly text: string }
	/** A quiet agent whose idle CLI `maxd` stopped, in muted words. */
	| { readonly kind: "asleep"; readonly text: string };

/**
 * The row's trailing text for the agent's state. `look` is its connection
 * dot's: a quiet agent that looks asleep says so in place of its time. One
 * with a turn you haven't read keeps the time, as unread comes first, and
 * one on a device that doesn't answer keeps it too, as the device's state
 * wins.
 */
export const rowMeta = (
	agent: Agent,
	children: readonly Agent[],
	now: Timestamp,
	look?: ConnectionLook
): RowMeta => {
	const activity = agent.activity;
	switch (activity.state) {
		case "idle":
		case "idle_cache_warm":
			return look === "asleep"
				? { kind: "asleep", text: t("Asleep") }
				: { kind: "text", text: formatAgoShort(agent.last_activity_at, now) };
		case "needs_input":
			return { kind: "input" };
		case "busy_in_subagents":
			return {
				kind: "text",
				text: t("{{value1}} sub", { value1: working(children).length || children.length }),
			};
		case "waiting_for_limit_reset":
			return {
				kind: "text",
				text:
					activity.resets_at !== null ? formatWhen(activity.resets_at, now) : t("Limit"),
			};
		case "starting":
			return { kind: "text", text: isResuming(agent, now) ? t("Resuming") : t("Starting") };
		case "paused":
			return { kind: "text", text: t("Paused") };
		default:
			return { kind: "text", text: formatAgoShort(agent.last_activity_at, now) };
	}
};

/**
 * {@link rowMeta} for an agent in the view, the one every row shows. It
 * shows Input too while a command you ran with `!` for the agent waits on
 * you.
 */
export const agentRowMeta = (view: ViewState, agent: Agent, now: Timestamp): RowMeta =>
	commandWaits(view, agent.id)
		? { kind: "input" }
		: rowMeta(
				agent,
				childrenOf(view, agent.id),
				now,
				agentConnection(agent, agentLinkContext(view, agent), now).look
			);

/** A sub-agent's status in the slide-out panel. */
export const subAgentStatus = (activity: Activity): string => {
	switch (activity.state) {
		case "busy":
			return t("Working");
		case "busy_in_subagents":
			return t("Waiting on its own");
		case "starting":
			return t("Starting");
		case "needs_input":
			return t("Needs input");
		case "done_unread":
		case "idle":
		case "idle_cache_warm":
			return t("Done");
		case "waiting_for_limit_reset":
			return t("Waiting for a limit");
		case "paused":
			return t("Paused");
		case "error":
			return t("Error");
		case "not_accessible":
			return t("Not accessible");
	}
};
