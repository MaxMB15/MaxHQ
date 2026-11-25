// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The More button on an agent's screen: what its sheet offers, the sheets
// its Permission, Model, Effort and Copy and export open, and the small
// dialog its Rename opens on the agent's name. The sheets list what the
// desktop's pickers and header menu list. Save sends the name through the
// shared renames, as the desktop's field does, and Cancel closes it with
// nothing sent.

import { t } from "@maxhq/shared/i18n";

import { createStore, type StoreApi } from "zustand/vanilla";

import { failureOf } from "@maxhq/shared/agent-view/answer";
import {
	canEnd,
	currentModel,
	modelLabel,
	moveModels,
	NOT_OFFERED,
	permissionChoices,
	permissionLabel,
	permissionNote,
	picksModel,
	runningMode,
	sessionIdName,
	STOP_AGENT,
} from "@maxhq/shared/agent-view/controls";
import {
	canSetEffort,
	type EffortChoice,
	effortChoices,
	effortLevel,
	effortShown,
} from "@maxhq/shared/agent-view/effort";
import { canMove } from "@maxhq/shared/agent-view/move-agent";
import type { AccountId, Agent, AgentPermission, PermissionMode } from "@maxhq/shared/bindings";
import { attempt } from "@maxhq/shared/composer/send";
import type { ActionResult } from "@maxhq/shared/data/source";
import { shownPulls } from "@maxhq/shared/pulls/links";
import { AUTO_NOTE, type AutoConsent, whenModeAllowed } from "@maxhq/shared/settings/auto-mode";
import { needsSwitch } from "@maxhq/shared/settings/phones";
import { accountName } from "@maxhq/shared/status/usage";
import { deletableAgents } from "@maxhq/shared/ui/delete-agents";
import { type Renames, renameAgent, shownTitle } from "@maxhq/shared/ui/rename-agent";
import type { ViewState } from "@maxhq/shared/view/replica";
import { type Move, modelsOf, movesOf } from "@maxhq/shared/view/select";

import type { Choice } from "../../ui/choose";
import { showsPorts } from "./running";
import { hasSubAgents } from "./sub-agents";

/** What the More sheet can offer. */
export type MoreChoice =
	| "find"
	| "prompts"
	| "rename"
	| "permission"
	| "model"
	| "effort"
	| "link"
	| "details"
	| "subAgents"
	| "running"
	| "copyOut"
	| "allowed"
	| "move"
	| "stop"
	| "delete";

/** Each choice as the sheet names it. */
export const MORE_LABEL: Record<MoreChoice, string> = {
	find: "Find",
	prompts: "Your prompts",
	rename: "Rename",
	permission: "Permission",
	model: "Model",
	effort: "Effort",
	link: "Link a pull request",
	details: "Details",
	subAgents: "Sub-agents",
	running: "Ports and processes",
	copyOut: "Copy and export…",
	allowed: "Allowed to run agents",
	move: "Move to…",
	stop: STOP_AGENT.label,
	delete: "Delete agent",
};

/**
 * The More sheet's choices: Find, Your prompts and Rename for every agent,
 * Permission, Model and Effort where the desktop's composer can change them,
 * Link a pull request while none is linked, Details, which the header's
 * second line also opens and holds the rest of the desktop's header,
 * Sub-agents for a lead that has some, Ports and processes once its folder
 * is known, Copy and export, Allowed to run agents while it `holds` a
 * permission, which the phone can revoke, Move for one that can move, Stop
 * agent while MaxHQ can end its process, and Delete for one MaxHQ holds.
 */
export const moreChoices = (view: ViewState, agent: Agent, holds = false): MoreChoice[] => {
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const choices: MoreChoice[] = ["find", "prompts", "rename"];
	if (agent.permission) choices.push("permission");
	if (picksModel(agent, modelsOf(view, account), movesOf(view, agent))) choices.push("model");
	if (account && canSetEffort(view, agent, account.device, modelsOf(view, account)))
		choices.push("effort");
	if (shownPulls(agent).length === 0) choices.push("link");
	choices.push("details");
	if (hasSubAgents(view, agent.id)) choices.push("subAgents");
	if (showsPorts(view, agent)) choices.push("running");
	choices.push("copyOut");
	if (holds) choices.push("allowed");
	if (canMove(view, agent)) choices.push("move");
	if (canEnd(agent)) choices.push("stop");
	if (deletableAgents(view, [agent.id]).length > 0) choices.push("delete");
	return choices;
};

/** What Copy and export offers. */
export type CopyOut = "session" | "file" | "copy";

/**
 * The Copy and export sheet, as the desktop's header menu offers it: Copy
 * the ID the CLI resumes the session by, when it has one, which the sheet's
 * line shows, then Export as Markdown and Copy as Markdown.
 */
export const copyOutSheet = (
	agent: Agent,
	pick: (what: CopyOut) => void
): { readonly message: string | undefined; readonly choices: readonly Choice[] } => {
	const id = agent.vendor_session_id;
	return {
		message: id ?? undefined,
		choices: [
			...(id
				? [
						{
							label: t("Copy {{sessionIdName}}", {
								sessionIdName: sessionIdName(agent),
							}),
							pick: () => pick("session"),
						},
					]
				: []),
			{ label: t("Export as Markdown…"), pick: () => pick("file") },
			{ label: t("Copy as Markdown"), pick: () => pick("copy") },
		],
	};
};

/** A sheet of choices, with a line under its title. */
export type Sheet = {
	readonly title: string;
	readonly message: string;
	readonly choices: readonly Choice[];
};

/** Why Auto and Full access are greyed out while the switch is off. */
const NEEDS_SWITCH = "Needs the switch";

/** The line that says where to turn the switch on. */
const SWITCH_LINE =
	"To pick Auto or Full access from the phone, turn on Let this phone do everything for this device in the Mac's Settings, under Phones.";

/**
 * The permission sheet: the modes the desktop's picker lists, with the one
 * the agent runs in, and those its CLI can't run in, greyed out. Unless the
 * agent's device `lets` this phone do everything, Auto and Full access are
 * greyed out too, since that device refuses them. A sheet shows no line
 * under each mode, so its message says what Auto runs, or where to turn
 * the switch on.
 */
export const permissionSheet = (
	view: ViewState,
	agent: Agent,
	permission: AgentPermission,
	pick: (mode: PermissionMode) => void,
	lets = true
): Sheet => {
	const running = runningMode(permission);
	const acp = agent.account ? view.tables.account.get(agent.account)?.acp : undefined;
	const choices = permissionChoices(agent.vendor, acp).map((choice) => ({
		...choice,
		reason: !choice.offered
			? NOT_OFFERED
			: !lets && needsSwitch(choice.mode)
				? NEEDS_SWITCH
				: null,
	}));
	const runs =
		permissionNote(permission) ?? t("Runs as {{mode}}.", { mode: permissionLabel(running) });
	const auto = choices.some((m) => m.mode === "auto" && m.reason === null);
	const held = choices.some((m) => m.reason === NEEDS_SWITCH);
	return {
		title: t("Permission mode"),
		message: [runs, ...(auto ? [t(AUTO_NOTE)] : []), ...(held ? [t(SWITCH_LINE)] : [])].join(
			" "
		),
		choices: choices.map(({ mode, label, reason }) => ({
			label: reason ? t("{{label}} ({{reason}})", { label, reason: t(reason) }) : label,
			disabled: reason !== null || mode === running,
			pick: () => pick(mode),
		})),
	};
};

/**
 * What picking a mode in the permission sheet does: `set` switches to it,
 * and Auto runs `ask` first, until you confirm it once on this phone.
 */
export const pickPermission =
	(consent: AutoConsent, ask: (yes: () => void) => void, set: (mode: PermissionMode) => void) =>
	(mode: PermissionMode): void =>
		whenModeAllowed(mode, consent, ask, () => set(mode));

/**
 * The model sheet: the models the agent's account can run, with the one
 * it runs greyed out and the default named, then a Move to each account it
 * can move to, which opens that account's models through `open`.
 */
export const modelSheet = (
	view: ViewState,
	agent: Agent,
	pick: (model: string, account?: AccountId) => void,
	open: (sheet: Sheet) => void
): Sheet => {
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const models = modelsOf(view, account);
	const current = currentModel(agent.model, models);
	const runs = modelLabel(agent.model, models);
	return {
		title: t("Model"),
		message: account
			? t("Runs {{model}} on {{account}}.", {
					model: runs,
					account: accountName(account, view.tables.model_server),
				})
			: t("Runs {{model}}.", { model: runs }),
		choices: [
			...models.map((choice) => ({
				label: choice.default
					? t("{{model}} (default)", { model: choice.label })
					: choice.label,
				disabled: choice.id === current,
				pick: () => pick(choice.id),
			})),
			...movesOf(view, agent).map((move) => ({
				label: t("Move to {{account}}", {
					account: accountName(move.account, view.tables.model_server),
				}),
				pick: () => open(moveSheet(view, move, pick)),
			})),
		],
	};
};

/** The effort dialog's levels, in order, and the one the agent runs at. */
export type EffortPick = {
	readonly message: string;
	readonly levels: readonly EffortChoice[];
	readonly current: string | null;
};

/**
 * The effort dialog: the levels the agent's model takes, in order, with the
 * one it runs at picked and the one its model runs at when told none
 * marked.
 */
export const effortPick = (view: ViewState, agent: Agent): EffortPick => {
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const models = modelsOf(view, account);
	const effort = agent.effort ?? null;
	return {
		message: t("Runs at {{level}}.", { level: effortShown(effort, agent.model, models) }),
		levels: effortChoices(effort, agent.model, models),
		current: effortLevel(effort, agent.model, models),
	};
};

/** The models of an account the agent can move to, taking its conversation along. */
export const moveSheet = (
	view: ViewState,
	move: Move,
	pick: (model: string, account?: AccountId) => void
): Sheet => ({
	title: t("Move to {{account}}", {
		account: accountName(move.account, view.tables.model_server),
	}),
	message: t("The conversation moves with the agent."),
	choices: moveModels(move).map((choice) => ({
		label: choice.label,
		pick: () => pick(choice.id, move.account.id),
	})),
});

/** What Stop agent asks first, in the desktop's words. */
export const stopQuestion = (
	title: string
): { readonly title: string; readonly detail: string } => ({
	title: t('Stop agent "{{title}}"?', { title }),
	detail: STOP_AGENT.detail,
});

/**
 * Runs one of the controls and resolves to what went wrong, as the
 * desktop's header says it, such as "Couldn't stop the agent. …", or null.
 */
export const runControl = async (
	what: string,
	action: () => Promise<ActionResult>
): Promise<string | null> => {
	const why = failureOf(await attempt(action));
	return why === null
		? null
		: t(
				(
					{
						"stop the agent": "Couldn't stop the agent. {{reason}}",
						"change the permission mode":
							"Couldn't change the permission mode. {{reason}}",
						"change the model": "Couldn't change the model. {{reason}}",
						"change the effort level": "Couldn't change the effort level. {{reason}}",
					} as Record<string, string>
				)[what] ?? "Couldn't {{action}}. {{reason}}",
				{ action: what, reason: why }
			);
};

/** The dialog: the agent it's open on, or null, and what's in its box. */
export type RenameDialogState = { readonly agent: Agent | null; readonly draft: string };

export type RenameDialog = {
	readonly store: StoreApi<RenameDialogState>;
	/** Opens on the agent's name as it shows. */
	readonly open: (agent: Agent) => void;
	readonly edit: (draft: string) => void;
	readonly cancel: () => void;
	/** Closes and renames the agent. Resolves to what went wrong, or null. */
	readonly save: () => Promise<string | null>;
};

/** A closed dialog that renames through `renames`. */
export const createRenameDialog = (renames: Renames): RenameDialog => {
	// Closing keeps the draft, so the box doesn't empty as the dialog fades.
	const store = createStore<RenameDialogState>(() => ({ agent: null, draft: "" }));
	return {
		store,
		open: (agent) =>
			store.setState({ agent, draft: shownTitle(renames.store.getState(), agent) }),
		edit: (draft) => store.setState({ draft }),
		cancel: () => store.setState({ agent: null }),
		save: async () => {
			const { agent, draft } = store.getState();
			if (!agent) return null;
			store.setState({ agent: null });
			return renameAgent(renames, agent, draft);
		},
	};
};
