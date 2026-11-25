// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatcher in words: which account it runs as, the one-line summary
// over a plan, each item's lines once you open it, how each item of a
// confirmed plan went, and the card of an agent or a task it started at
// once. Nothing here changes the view.

import { t } from "../i18n";

import type {
	Account,
	AccountId,
	AgentId,
	AgentPlace,
	DeviceId,
	Filter,
	HookRules,
	PermissionMode,
	Plan,
	PlanItem,
	PlanOutcome,
	PlanStep,
	PlanTarget,
	TimelineItem,
	Trigger,
	UserInput,
} from "../bindings";
import { labeledText } from "../pills/draft";
import { agentMode, MODE_LABEL, movedMode, startingMode } from "../settings/permission";
import { needsSwitch } from "../settings/phones";
import { listInWords, plural, sentence } from "../status/format";
import { agentKindName, isLocalAccount } from "../status/agent-kind";
import { accountName } from "../status/usage";
import { FILTER_OPS } from "../tasks/draft";
import { triggerLine } from "../tasks/format";
import { tasksTargeting } from "../tasks/place";
import type { ViewState } from "../view/replica";
import { accounts, deviceName, workspaceName } from "../view/select";

import { type ActionCard, adminPhrase, cardUndo, grantLines, settingLines } from "./action";

import { nameOf } from "../folders/path";
/**
 * The accounts on `device` the dispatcher can run as: Claude first, then
 * Codex, then local models, which it never picks unless you do.
 */
export const dispatcherAccounts = (view: ViewState, device: DeviceId | null): Account[] =>
	accounts(view).filter(
		(a) => a.device === device && (a.vendor === "claude" || a.vendor === "codex")
	);

/**
 * The account the dispatcher runs as: the one picked in settings while
 * `device` still has it, else the first Claude account, else the first
 * Codex one. A local model runs it only when picked.
 */
export const dispatcherAccount = (
	view: ViewState,
	device: DeviceId | null,
	picked: AccountId | null
): Account | undefined => {
	const choices = dispatcherAccounts(view, device);
	return choices.find((a) => a.id === picked) ?? choices.find((a) => !isLocalAccount(a));
};

/** An account as the pick in settings names it, such as "Claude Max, octocat@example.com". */
export const dispatcherAccountLabel = (account: Account): string =>
	[accountName(account), account.email].filter(Boolean).join(", ");

/** Over the dispatcher's conversation, such as "Dispatcher, as Claude Max". */
export const dispatcherTitle = (account: Account | undefined): string =>
	account
		? t("Dispatcher, as {{accountName_account}}", { accountName_account: accountName(account) })
		: t("Dispatcher");

/** A project's name where it has the place, else the folder or container's own name. */
const placeName = (view: ViewState, device: DeviceId, place: AgentPlace): string => {
	if (place.type === "workspace") {
		const workspace = view.tables.workspace.get(place.workspace);
		if (!workspace) return t("a workspace");
		const project = workspace.project ? view.tables.project.get(workspace.project) : undefined;
		return project?.name ?? workspaceName(workspace);
	}
	for (const workspace of view.tables.workspace.values()) {
		if (
			workspace.device === device &&
			workspace.location.type === "folder" &&
			workspace.location.path === place.path &&
			workspace.project
		) {
			const project = view.tables.project.get(workspace.project);
			if (project) return project.name;
		}
	}
	return nameOf(place.path);
};

/** Where an opened item runs: a folder's whole path, else the place's name. */
const whereLine = (view: ViewState, device: DeviceId, place: AgentPlace): string =>
	place.type === "folder" ? place.path : placeName(view, device, place);

const agentTitle = (view: ViewState, agent: string): string =>
	view.tables.agent.get(agent)?.title ?? t("an agent");

/** What makes a planned task run, short enough for the summary. */
const triggerPhrase = (trigger: Trigger): string => {
	switch (trigger.type) {
		case "github_events": {
			const prs = trigger.events.every((e) => e.startsWith("pull_request"));
			return t("on {{value1}} {{value2}}", {
				value1: trigger.repo,
				value2: prs ? t("PR events") : "events",
			});
		}
		case "hook":
			return t("on a webhook");
		default:
			return t("that runs {{triggerLine_trigger__toLowerCase}}", {
				triggerLine_trigger__toLowerCase: triggerLine(trigger).toLowerCase(),
			});
	}
};

/**
 * The mode an item's new agent runs in, or null when the item starts none.
 * A moved agent takes the lower of its mode now and the default where it
 * goes, unless the plan names the higher one.
 */
export const itemMode = (view: ViewState, item: PlanItem): PermissionMode | null => {
	const { step } = item;
	if (step.type === "start_agent") {
		return step.spec.permission ?? startingMode(view, step.device, step.spec.vendor);
	}
	if (step.type === "move_agent") {
		const vendor = view.tables.account.get(step.to.account)?.vendor;
		const agent = view.tables.agent.get(step.agent);
		return vendor && agent
			? movedMode(
					view,
					step.to.device,
					vendor,
					agentMode(view, agent, step.device),
					step.to.mode
				)
			: (step.to.mode ?? null);
	}
	if (step.type !== "schedule_task" || step.target.type !== "task") return null;
	const aim = step.target.target;
	if (aim.type !== "new") return null;
	return aim.spec.permission ?? startingMode(view, aim.device, aim.spec.vendor);
};

const names = (mode?: PermissionMode | null): boolean => !!mode && needsSwitch(mode);

/**
 * Whether a step names Auto or Full access, so a phone confirms it only
 * when the dispatcher's device lets it do everything, as that maxd checks:
 * a start's mode, a new task agent's, a move's, an agent's new mode or the
 * device's new default. A start that names no mode runs in the default the
 * Mac set, so it doesn't count.
 */
export const stepNeedsSwitch = (step: PlanStep): boolean => {
	switch (step.type) {
		case "start_agent":
			return names(step.spec.permission);
		case "schedule_task":
			return (
				step.target.type === "task" &&
				step.target.target.type === "new" &&
				names(step.target.target.spec.permission)
			);
		case "move_agent":
			return names(step.to.mode);
		case "admin": {
			const { action } = step;
			if (action.action === "set_mode") return needsSwitch(action.mode);
			return (
				action.action === "change_settings" &&
				action.changes.some((c) => c.field === "default_permission" && needsSwitch(c.value))
			);
		}
		case "send_message":
			return false;
	}
};

/** How many lines of what an item sends show before you open it. */
const PREVIEW_LINES = 3;
/** How much of what an item sends shows before you open it. */
const PREVIEW_CHARS = 400;

// A first message with each pill as its words. The plan carries no files.
const firstText = (first: UserInput | null): string | undefined =>
	first ? labeledText(first, () => undefined) : undefined;

// What a step sends: a new agent's first message, a message or a task's
// prompt. A move or a change to MaxHQ sends nothing.
const sentText = (step: PlanStep): string | null | undefined => {
	switch (step.type) {
		case "start_agent":
			return firstText(step.spec.first);
		case "send_message":
			return step.text;
		case "schedule_task":
			return step.prompt;
		case "move_agent":
		case "admin":
			return null;
	}
};

/**
 * The first lines of what an item sends, without blank ones so that nothing
 * hides further down. It ends in dots when there's more to read.
 */
export const itemPreview = (item: PlanItem): string | null => {
	const lines = (sentText(item.step) ?? "").split("\n").filter((line) => line.trim() !== "");
	if (lines.length === 0) return null;
	const shown = lines.slice(0, PREVIEW_LINES).join("\n");
	const cut = shown.slice(0, PREVIEW_CHARS);
	return lines.length > PREVIEW_LINES || cut.length < shown.length ? `${cut}...` : cut;
};

/** One item in the words of the summary, such as "a Cursor agent in Comet on MacBook Pro". */
export const itemPhrase = (view: ViewState, item: PlanItem): string => {
	const { step } = item;
	const access = itemMode(view, item) === "full_access" ? t(", with Full access") : "";
	switch (step.type) {
		case "start_agent": {
			const { spec } = step;
			const who =
				spec.vendor === "acp"
					? (view.tables.account.get(spec.account)?.label ?? "local")
					: agentKindName(spec, view.tables.account);
			return t(
				"a {{who}} agent in {{placeName_view__step_device__spec_place}} on {{deviceName_view__step_device}}{{access}}",
				{
					who: who,
					placeName_view__step_device__spec_place: placeName(
						view,
						step.device,
						spec.place
					),
					deviceName_view__step_device: deviceName(view, step.device),
					access: access,
				}
			);
		}
		case "send_message":
			return t("a message to {{agentTitle_view__step_agent}}", {
				agentTitle_view__step_agent: agentTitle(view, step.agent),
			});
		case "schedule_task":
			return t("a task {{triggerPhrase_step_trigger}}{{access}}", {
				triggerPhrase_step_trigger: triggerPhrase(step.trigger),
				access: access,
			});
		case "move_agent":
			return t("a move of {{agent}} to {{device}}{{access}}", {
				agent: agentTitle(view, step.agent),
				device: deviceName(view, step.to.device),
				access: access,
			});
		case "admin":
			return adminPhrase(view, step.device, step.action);
	}
};

/**
 * The line over a plan, such as "2 items: a Cursor agent in Comet on
 * MacBook Pro and a task on owner/repo PR events".
 */
export const planSummary = (view: ViewState, plan: Plan): string =>
	`${plural(plan.items.length, "item")}: ${listInWords(plan.items.map((i) => itemPhrase(view, i)))}`;

/** One labelled line of an opened item. */
export type ItemLine = { readonly label: string; readonly value: string };

const filterLine = (filter: Filter): string => {
	const op = FILTER_OPS.find((o) => o.op === filter.test.op)?.label ?? filter.test.op;
	const { test } = filter;
	if (test.op === "exists") return `${filter.path} ${op}`;
	const value = test.op === "in" ? test.values.join(", ") : test.value;
	return `${filter.path} ${op} ${value}`;
};

/**
 * A task's filters and checks, each as a line, leaving out what's unset.
 * Its wait is in the line that says when it runs.
 */
const ruleLines = (rules: HookRules): ItemLine[] => {
	const lines: ItemLine[] = [];
	for (const filter of rules.filters)
		lines.push({ label: t("Only when"), value: filterLine(filter) });
	if ((rules.authors ?? []).length > 0)
		lines.push({
			label: t("Only for"),
			value: t("The repo's owners, members and collaborators"),
		});
	if (rules.replay_key !== null)
		lines.push({
			label: t("Repeats"),
			value: t("Skips a repeat of {{rules_replay_key}}", {
				rules_replay_key: rules.replay_key,
			}),
		});
	if (rules.max_age_minutes !== null)
		lines.push({
			label: t("Too old"),
			value: t("Skips events older than {{plural_rules_max_age_minutes___min____min}}", {
				plural_rules_max_age_minutes___min____min: plural(
					rules.max_age_minutes,
					"min",
					"min"
				),
			}),
		});
	if (rules.daily_runs !== null)
		lines.push({
			label: t("Each day"),
			value: t("At most {{plural_rules_daily_runs___run}}", {
				plural_rules_daily_runs___run: plural(rules.daily_runs, "run"),
			}),
		});
	return lines;
};

const targetLine = (view: ViewState, plan: Plan, target: PlanTarget): string => {
	if (target.type === "item") {
		const started = plan.items[target.item];
		return started
			? t("The new {{itemPhrase_view__started__replace___a}}", {
					itemPhrase_view__started__replace___a: itemPhrase(view, started).replace(
						/^a /,
						""
					),
				})
			: t("An earlier item's agent");
	}
	const aim = target.target;
	if (aim.type === "agent") return agentTitle(view, aim.agent);
	const where = placeName(view, aim.device, aim.spec.place);
	return t("A new {{VENDOR_NAME_aim_spec_vendor}} agent for each run, in {{where}}", {
		VENDOR_NAME_aim_spec_vendor: agentKindName(aim.spec, view.tables.account),
		where: where,
	});
};

/** Everything an opened item shows, filters and wait included. */
export const itemLines = (view: ViewState, plan: Plan, index: number): ItemLine[] => {
	const item = plan.items[index];
	if (!item) return [];
	const { step } = item;
	const mode = itemMode(view, item);
	const modeLine = mode ? [{ label: t("Mode"), value: MODE_LABEL[mode] }] : [];
	switch (step.type) {
		case "start_agent": {
			const { spec } = step;
			const account = view.tables.account.get(spec.account);
			return [
				{
					label: t("Account"),
					value: account?.label ?? agentKindName(spec, view.tables.account),
				},
				{ label: t("Device"), value: deviceName(view, step.device) },
				{ label: t("Where"), value: whereLine(view, step.device, spec.place) },
				{ label: t("Model"), value: spec.model ?? t("Default") },
				...modeLine,
				...(spec.title ? [{ label: t("Title"), value: spec.title }] : []),
				{
					label: t("First message"),
					value: firstText(spec.first) ?? t("None, it starts idle"),
				},
			];
		}
		case "send_message":
			return [
				{ label: t("Agent"), value: agentTitle(view, step.agent) },
				{ label: t("Message"), value: step.text },
			];
		case "schedule_task": {
			const { trigger } = step;
			const rules =
				trigger.type === "github_events" || trigger.type === "hook" ? trigger.rules : null;
			return [
				{ label: t("Name"), value: step.name },
				{ label: t("Device"), value: deviceName(view, step.device) },
				{ label: t("Runs"), value: triggerLine(trigger) },
				...(rules ? ruleLines(rules) : []),
				{ label: t("Sends to"), value: targetLine(view, plan, step.target) },
				...modeLine,
				{ label: t("Prompt"), value: step.prompt },
			];
		}
		case "move_agent": {
			const { to } = step;
			const account = view.tables.account.get(to.account);
			const follow = tasksTargeting(view, step.agent).length;
			return [
				{ label: t("Agent"), value: agentTitle(view, step.agent) },
				{ label: t("From"), value: deviceName(view, step.device) },
				{ label: t("To"), value: deviceName(view, to.device) },
				{
					label: t("Account"),
					value: account ? dispatcherAccountLabel(account) : t("An account there"),
				},
				{ label: t("Model"), value: to.model ?? t("Default") },
				{ label: t("Where"), value: whereLine(view, to.device, to.place) },
				...modeLine,
				{ label: t("Carries over"), value: t("Only a summary of the conversation") },
				...(follow > 0 ? [{ label: t("Automations"), value: followLine(follow) }] : []),
			];
		}
		case "admin":
			return [
				{ label: t("Device"), value: deviceName(view, step.device) },
				{
					label: t("Change"),
					value: sentence(adminPhrase(view, step.device, step.action)),
				},
				...settingLines(step.action).map((value) => ({ label: t("Setting"), value })),
				...grantLines(view, step.action),
			];
	}
};

/** The automations that follow a moved agent, such as "2 automations will follow it". */
export const followLine = (count: number): string =>
	t("{{count, number}} automations will follow it", { count });

/** How a confirmed item went, in a word or with why it failed. */
export const outcomeLine = (outcome: PlanOutcome): string => {
	switch (outcome.type) {
		case "started":
			return t("Started");
		case "sent":
			return t("Sent");
		case "task_saved":
			return t("Task saved");
		case "done":
			return t("Done");
		case "failed":
			return outcome.reason;
		case "unknown":
			return t("Done; update the app for details");
	}
};

/** An agent or a task the dispatcher started or saved at once, as its card. */
export type StepItem = Extract<TimelineItem, { kind: "dispatcher_step" }>;

/**
 * The card for an agent or a task the dispatcher started or saved at once,
 * such as "Started a Claude agent in Comet on Home Mac". Its line says
 * where, so it names no device of its own.
 */
export const stepCard = (view: ViewState, item: StepItem): ActionCard => {
	const { step } = item;
	const what = itemPhrase(view, { reason: "", step });
	return {
		line:
			step.type === "schedule_task"
				? t("Scheduled {{name}}, {{what}}", { name: step.name, what })
				: t("Started {{what}}", { what }),
		elsewhere: null,
		...cardUndo(view, item.agent, item.undo),
	};
};

/** Under an item when a failed one before it stopped the plan. */
export const NOT_RUN = "Not run, an earlier item failed";

/** The line over a plan that ran, or that stopped at the item that failed. */
export const resultsTitle = (results: readonly PlanOutcome[]): string => {
	const failed = results.findIndex((r) => r.type === "failed");
	return failed < 0
		? t("The plan ran")
		: t("The plan stopped at item {{failed___1}}", { failed___1: failed + 1 });
};

/** One item of a plan that ran: what it was, how it went, and what it made. */
export type ResultRow = {
	/** Such as "A Cursor agent in Comet on MacBook Pro". */
	readonly phrase: string;
	/** How it went, or that it didn't run. */
	readonly line: string;
	readonly failed: boolean;
	/** The agent it started or sent to. */
	readonly agent: AgentId | null;
	/** The name of the task it saved. */
	readonly task: string | null;
};

/** Each item of a plan that ran, in order. */
export const resultRows = (
	view: ViewState,
	plan: Plan,
	results: readonly PlanOutcome[]
): ResultRow[] =>
	plan.items.map((item, index) => {
		const outcome = results[index];
		const moved = item.step.type === "move_agent" && outcome?.type === "started";
		return {
			phrase: sentence(itemPhrase(view, item)),
			line: moved ? t("Moved") : outcome ? outcomeLine(outcome) : NOT_RUN,
			failed: outcome?.type === "failed",
			agent: outcome?.type === "started" || outcome?.type === "sent" ? outcome.agent : null,
			task:
				outcome?.type === "task_saved" && item.step.type === "schedule_task"
					? item.step.name
					: null,
		};
	});
