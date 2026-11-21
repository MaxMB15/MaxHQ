// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the schedule label,
// the relative time label, the outcome badges and the webhook delivery label. The
// words a task shows in lists: what makes it run, when it runs next, how
// its last run went and what came of each request. GitHub events, the hold
// and the runs' own outcomes are new, as are the webhook address words and
// how a GitHub events task hears of events.

import { formatNumber, t } from "../i18n";

import type {
	DeliveryOutcome,
	DeviceTasks,
	FunnelState,
	HookAddress,
	HookRules,
	RunCause,
	RunOutcome,
	RunSummary,
	Task,
	TaskDelivery,
	TaskId,
	Timestamp,
	Trigger,
	Weekday,
} from "../bindings";
import { parseTime } from "../lib/zoned-time";
import { formatAgo, listInWords } from "../status/format";
import { COALESCE_SECS, MAX_WAIT_SECS, type PromptField, WEEKDAYS } from "./draft";

const MINUTE = 60_000;
const pad = (n: number) => String(n).padStart(2, "0");

/** What makes a task run, such as "Every 15 min" or "Weekdays at 09:00". */
export const triggerLine = (trigger: Trigger, zone?: string): string => {
	switch (trigger.type) {
		case "every":
			return trigger.minutes % 60 === 0
				? trigger.minutes === 60
					? t("Every hour")
					: t("Every {{trigger_minutes___60}} hours", {
							trigger_minutes___60: trigger.minutes / 60,
						})
				: t("Every {{trigger_minutes}} min", { trigger_minutes: trigger.minutes });
		case "at": {
			const days = trigger.days;
			const workdays = WEEKDAYS.slice(0, 5).map((w) => w.day);
			const when =
				days.length === 0 || days.length === 7
					? t("Daily")
					: days.length === 5 && workdays.every((d) => days.includes(d))
						? t("Weekdays")
						: WEEKDAYS.filter((w) => days.includes(w.day))
								.map((w) => w.label)
								.join(", ");
			const time = `${pad(trigger.time.hour)}:${pad(trigger.time.minute)}`;
			// The zone shows only when it isn't the one you're in.
			return t("{{value1}} at {{value2}}{{value3}}", {
				value1: when,
				value2: time,
				value3: zone && trigger.zone !== zone ? ` ${trigger.zone}` : "",
			});
		}
		case "hook":
			return waited(t("On webhook"), trigger.rules, "request");
		case "github_events":
			return waited(
				t("On {{listInWords_trigger_events_map_githubEventLab}} in {{trigger_repo}}", {
					listInWords_trigger_events_map_githubEventLab: listInWords(
						trigger.events.map(githubEventLabel)
					),
					trigger_repo: trigger.repo,
				}),
				trigger.rules,
				"event"
			);
	}
};

/**
 * The hint under an At task's time picker, which shows the clock `here` is
 * on: the zone the task runs in, and when it runs there once that isn't
 * `here`, such as "In Europe/Amsterdam: Mon at 23:00".
 */
export const zoneHint = (
	at: { readonly time: string; readonly days: ReadonlySet<Weekday>; readonly zone: string },
	here: string
): string => {
	const time = parseTime(at.time);
	if (at.zone === here || !time)
		return t("In {{zone}}. No days picked runs it every day.", { zone: at.zone });
	return t("In {{zone}}: {{schedule}}. No days picked runs it every day.", {
		zone: at.zone,
		schedule: triggerLine({ type: "at", time, days: [...at.days], zone: at.zone }),
	});
};

/** Seconds in a few words, such as "15 min" or "2 h". */
const span = (secs: number): string =>
	secs % 3600 === 0
		? t("{{count, number}} h", { count: secs / 3600 })
		: secs % 60 === 0
			? t("{{count, number}} min", { count: secs / 60 })
			: t("{{count, number}} s", { count: secs });

/**
 * How long a task that coalesces waits to run, such as "Waits 15 min
 * after the last event, 2 h at most", or null when each request runs it.
 */
export const waitLine = (rules: HookRules, what: "request" | "event"): string | null => {
	if (rules.coalesce_key === null) return null;
	const window = span(rules.coalesce_secs ?? COALESCE_SECS.default);
	if (!rules.coalesce_restart)
		return t("Waits {{window}} after the first {{what}}", { window: window, what: t(what) });
	const most = span(rules.coalesce_max_secs ?? MAX_WAIT_SECS.default);
	return t("Waits {{window}} after the last {{what}}, {{most}} at most", {
		window: window,
		what: t(what),
		most: most,
	});
};

const waited = (line: string, rules: HookRules, what: "request" | "event"): string => {
	const wait = waitLine(rules, what);
	return wait ? `${line}. ${wait}` : line;
};

/** How long until a time, such as "in 5m", or how long ago it was. */
export const relativeLine = (at: Timestamp, now: Timestamp): string => {
	if (at <= now) return formatAgo(at, now);
	const minutes = Math.ceil((at - now) / MINUTE);
	if (minutes < 2) return t("in under a minute");
	if (minutes < 60) return t("in {{minutes}}m", { minutes: minutes });
	const hours = Math.round(minutes / 60);
	if (hours < 24) return t("in {{hours}}h", { hours: hours });
	return t("in {{Math_round_hours___24}}d", { Math_round_hours___24: Math.round(hours / 24) });
};

/** Who made a task when it wasn't you: the dispatcher at once, or an agent. */
export const makerLine = (task: Task): string | null => {
	if (task.made_by.type === "tools") return t("Made by an agent");
	return task.by_dispatcher ? t("Made by the dispatcher") : null;
};

/** When a task runs next, or why it waits, such as "Next run in 5m" or "Paused". */
export const nextRunLine = (task: Task, now: Timestamp): string => {
	if (task.approval_asked) return t("Waits for your approval");
	if (!task.enabled) return t("Paused");
	switch (task.trigger.type) {
		case "hook":
			return task.trigger.on ? t("Listening") : t("Webhook off");
		case "github_events":
			return t("Watching {{task_trigger_repo}}", { task_trigger_repo: task.trigger.repo });
		default:
			return task.next_run !== null
				? t("Next run {{relativeLine_task_next_run__now}}", {
						relativeLine_task_next_run__now: relativeLine(task.next_run, now),
					})
				: t("Not scheduled");
	}
};

/**
 * How an enabled GitHub events task hears of events, such as "Instant
 * through a webhook. Last delivery 5 min ago", or null for any other task
 * and on a `maxd` from before webhooks.
 */
export const webhookLine = (
	task: Task,
	reach: Pick<DeviceTasks, "webhooks">,
	now: Timestamp
): string | null => {
	if (task.trigger.type !== "github_events" || !task.enabled || !reach.webhooks) return null;
	const webhook = reach.webhooks.find((w) => w.task === task.id);
	const how = webhook?.instant ? t("Instant through a webhook") : t("Checks every minute");
	const last = webhook?.last_delivery;
	return last != null
		? t("{{value1}}. Last delivery {{value2}}", { value1: how, value2: formatAgo(last, now) })
		: how;
};

/** How a run went, in a word. */
export const RUN_OUTCOME: Record<RunOutcome, string> = {
	get running() {
		return t("Running");
	},
	get done() {
		return t("Ran");
	},
	get failed() {
		return t("Failed");
	},
	get stopped() {
		return t("Stopped");
	},
	get missed() {
		return t("Missed");
	},
};

/** A run's tone, for its badge. */
export type Tone = "good" | "warn" | "bad" | "info" | "plain";

/** The tone of a run's outcome. */
export const runTone = (outcome: RunOutcome): Tone =>
	outcome === "done"
		? "good"
		: outcome === "failed"
			? "bad"
			: outcome === "running"
				? "info"
				: outcome === "missed"
					? "warn"
					: "plain";

/** How the last run went, such as "Ran 5 min ago", or "No runs yet". */
export const lastRunLine = (last: RunSummary | null, now: Timestamp): string => {
	if (!last) return t("No runs yet");
	if (last.outcome === "running") return t("Running now");
	return `${RUN_OUTCOME[last.outcome]} ${formatAgo(last.at, now)}`;
};

/** What made a run happen. */
export const RUN_CAUSE: Record<RunCause, string> = {
	get timer() {
		return t("On time");
	},
	get now() {
		return t("Run now");
	},
	get hook() {
		return t("Webhook");
	},
	get github() {
		return t("GitHub event");
	},
};

const DELIVERY_LABEL: Record<DeliveryOutcome, string> = {
	get accepted() {
		return t("Ran");
	},
	get dispatch_failed() {
		return t("Run failed");
	},
	get duplicate() {
		return t("Duplicate");
	},
	get coalesced() {
		return t("Coalesced");
	},
	get rejected_signature() {
		return t("Bad signature");
	},
	get disabled() {
		return t("Task paused");
	},
	get rate_limited() {
		return t("Rate limited");
	},
	get expired() {
		return t("Too old");
	},
	get filtered() {
		return t("Filtered");
	},
	get prompt_too_long() {
		return t("Prompt too long");
	},
	get queue_full() {
		return t("Queue full");
	},
	get held() {
		return t("Held");
	},
	get target_offline() {
		return t("Target offline");
	},
};

/** What came of a request, such as "Ran" or "Bad signature". */
export const deliveryLabel = (outcome: DeliveryOutcome): string => DELIVERY_LABEL[outcome];

/** A delivery's tone, for its badge. */
export const deliveryTone = (outcome: DeliveryOutcome): Tone => {
	if (outcome === "accepted") return "good";
	if (outcome === "dispatch_failed" || outcome === "rejected_signature") return "bad";
	if (outcome === "held") return "info";
	if (outcome === "filtered") return "plain";
	return "warn";
};

/** How a request came, such as its method, content type, size and signature. */
export const deliveryFacts = (delivery: TaskDelivery): string[] =>
	[
		delivery.method,
		delivery.content_type,
		t("{{value1}} bytes", { value1: formatNumber(delivery.body_bytes) }),
		delivery.signature_verified ? "signed" : null,
	].filter((fact): fact is string => Boolean(fact));

/**
 * The GitHub events a task can watch, by their webhook names: the ones
 * `maxd` maps from the Events API, less `public` and `sponsorship`, which
 * a repo's work doesn't raise.
 */
export const GITHUB_EVENTS: readonly { readonly event: string; readonly label: string }[] = [
	{
		event: "push",
		get label() {
			return t("Pushes");
		},
	},
	{
		event: "pull_request",
		get label() {
			return t("Pull requests");
		},
	},
	{
		event: "pull_request_review",
		get label() {
			return t("Reviews");
		},
	},
	{
		event: "pull_request_review_comment",
		get label() {
			return t("Review comments");
		},
	},
	{
		event: "pull_request_review_thread",
		get label() {
			return t("Review threads");
		},
	},
	{
		event: "issues",
		get label() {
			return t("Issues");
		},
	},
	{
		event: "issue_comment",
		get label() {
			return t("Comments");
		},
	},
	{
		event: "commit_comment",
		get label() {
			return t("Commit comments");
		},
	},
	{
		event: "release",
		get label() {
			return t("Releases");
		},
	},
	{
		event: "create",
		get label() {
			return t("Branches and tags made");
		},
	},
	{
		event: "delete",
		get label() {
			return t("Branches and tags deleted");
		},
	},
	{
		event: "fork",
		get label() {
			return t("Forks");
		},
	},
	{
		event: "watch",
		get label() {
			return t("Stars");
		},
	},
	{
		event: "member",
		get label() {
			return t("Collaborators");
		},
	},
	{
		event: "gollum",
		get label() {
			return t("Wiki edits");
		},
	},
];

/** An event's name in words, such as "pull requests". */
export const githubEventLabel = (event: string): string =>
	GITHUB_EVENTS.find((e) => e.event === event)?.label.toLowerCase() ?? event;

/** How events reach a task, said under the event picker. */
export const GITHUB_LATENCY_NOTE =
	"When its device's webhooks take requests from elsewhere, maxd sets up a webhook on the repository through the GitHub App. Pushes and PR events then come at once, while other events make maxd check GitHub's events API sooner. Otherwise it checks the events API every minute. The events API shows events 30 seconds to 6 hours after they happen.";

/** How a device's webhooks are reached, in words. */
export const ADDRESS_LABEL: Record<HookAddress["kind"], string> = {
	get local() {
		return t("This device only");
	},
	get funnel() {
		return t("Tailscale Funnel");
	},
	get own() {
		return t("Your own address");
	},
};

const FUNNEL_HINT: Record<FunnelState["state"], string> = {
	get off() {
		return t("Funnel off");
	},
	get on() {
		return t("Funnel on");
	},
	get needs_setup() {
		return t("Funnel needs setting up");
	},
};

/** How a device's webhooks are reached. A `maxd` from before the choice reaches them only on itself. */
export const addressOf = (reach: Pick<DeviceTasks, "address">): HookAddress =>
	reach.address ?? { kind: "local" };

/** A device's webhook address and, on Funnel, whether it works, in a few words. */
export const addressHint = (address: HookAddress, funnel: FunnelState): string =>
	address.kind === "funnel" ? FUNNEL_HINT[funnel.state] : ADDRESS_LABEL[address.kind];

/**
 * Who reaches a device's webhooks, in a sentence, for `device` by its
 * name, or null while its Funnel needs setting up, which shows the steps.
 */
export const addressNote = (
	reach: Pick<DeviceTasks, "address" | "port" | "funnel">,
	device: string
): string | null => {
	const { port, funnel } = reach;
	const address = addressOf(reach);
	if (address.kind === "local")
		return t(
			"Only programs on {{device}} reach its webhooks. Pick another address in Settings › Scheduled tasks to take requests from elsewhere.",
			{ device: device }
		);
	if (address.kind === "own")
		return t(
			"Requests come in through {{base}}. maxd can't check that it reaches{{value4}}{{value6}} on {{device}}.",
			{
				base: address.base,
				value4: " ",
				value6: port
					? t("port {{port}}", { port: formatNumber(port, { useGrouping: false }) })
					: t("the hooks port"),
				device: device,
			}
		);
	if (funnel.state === "on")
		return t("Funnel is on, so requests reach {{device}}.", { device: device });
	if (funnel.state === "off")
		return t("Funnel turns on for {{device}} when a webhook on it does.", { device: device });
	return null;
};

/** What to paste as your own address, for a device whose hooks listen on `port`. */
export const ownAddressHint = (port: number | null | undefined): string =>
	t(
		"Paste the address of a tunnel or proxy that points at{{value2}}{{value4}}. It needs https, or http on this device. maxd can't check that it works.",
		{
			value2: " ",
			value4: port ? `http://127.0.0.1:${port}` : t("this device's hooks port"),
		}
	);

/** How a prompt's fields fill in, said under them, for GitHub events or a webhook. */
export const fieldsNote = (github: boolean): string =>
	github
		? t(
				"Numbers, logins, actions, URLs and other structural values fill in as they are, and fenced when one doesn't look the way GitHub writes it. Free text, such as a title, a body or a comment, comes from whoever wrote it, so it stays out unless you allow it, and then fills in fenced as untrusted data."
			)
		: t(
				"Every value from the request, headers and query included, comes from whoever sent it, so it stays out unless you allow it, and then fills in fenced as untrusted data."
			);

/** What a prompt's field is: free text, or a structural value. */
export const fieldKind = ({ expression, free }: PromptField, github: boolean): string =>
	free
		? t("Free text")
		: github && expression === "body"
			? t("Structural parts")
			: t("Structural");

/**
 * A webhook's URL with its token hidden, since only a rotate shows it
 * again. Before the address works, it's only the path.
 */
export const hiddenHookUrl = (task: TaskId, funnel: FunnelState): string =>
	`${funnel.state === "on" ? funnel.url : ""}/maxhq/${task}/••••••`;

/** Whether a task's webhook waits on a signing secret its device doesn't hold. */
export const needsSecret = (task: Task): boolean =>
	task.trigger.type === "hook" &&
	task.trigger.signature !== null &&
	!task.trigger.signature.has_secret;
