// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The More button on an agent's screen: what its sheet offers, the sheets
// its Permission, Model and Effort open, and the small dialog its Rename opens on
// the agent's name. The sheets list what the desktop's pickers list. Save
// sends the name through the shared renames, as the desktop's field does,
// and Cancel closes it with nothing sent.

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
	STOP_AGENT,
} from "@maxhq/shared/agent-view/controls";
import {
	canSetEffort,
	type EffortChoice,
	effortChoices,
	effortLevel,
	effortShown,
} from "@maxhq/shared/agent-view/effort";
import type { AccountId, Agent, AgentPermission, PermissionMode } from "@maxhq/shared/bindings";
import { attempt } from "@maxhq/shared/composer/send";
import type { ActionResult } from "@maxhq/shared/data/source";
import { shownPulls } from "@maxhq/shared/pulls/links";
import { AUTO_NOTE, type AutoConsent, whenModeAllowed } from "@maxhq/shared/settings/auto-mode";
import { accountName } from "@maxhq/shared/status/usage";
import { deletableAgents } from "@maxhq/shared/ui/delete-agents";
import { type Renames, renameAgent, shownTitle } from "@maxhq/shared/ui/rename-agent";
import type { ViewState } from "@maxhq/shared/view/replica";
import { type Move, modelsOf, movesOf } from "@maxhq/shared/view/select";

import type { Choice } from "../../ui/choose";

/** What the More sheet can offer. */
export type MoreChoice =
	"find" | "rename" | "permission" | "model" | "effort" | "link" | "stop" | "delete";

/** Each choice as the sheet names it. */
export const MORE_LABEL: Record<MoreChoice, string> = {
	find: "Find",
	rename: "Rename",
	permission: "Permission",
	model: "Model",
	effort: "Effort",
	link: "Link a pull request",
	stop: STOP_AGENT.label,
	delete: "Delete agent",
};

/**
 * The More sheet's choices: Find and Rename for every agent, Permission,
 * Model and Effort where the desktop's composer can change them, Link a pull
 * request while none is linked, as the desktop's header menu offers it, Stop
 * agent while MaxHQ can end its process, and Delete for one MaxHQ holds.
 */
export const moreChoices = (view: ViewState, agent: Agent): MoreChoice[] => {
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const choices: MoreChoice[] = ["find", "rename"];
	if (agent.permission) choices.push("permission");
	if (picksModel(agent, modelsOf(view, account), movesOf(view, agent))) choices.push("model");
	if (account && canSetEffort(view, agent, account.device, modelsOf(view, account)))
		choices.push("effort");
	if (shownPulls(agent).length === 0) choices.push("link");
	if (canEnd(agent)) choices.push("stop");
	if (deletableAgents(view, [agent.id]).length > 0) choices.push("delete");
	return choices;
};

/** A sheet of choices, with a line under its title. */
export type Sheet = {
	readonly title: string;
	readonly message: string;
	readonly choices: readonly Choice[];
};

/**
 * The permission sheet: the modes the desktop's picker lists, with the one
 * the agent runs in, and those its CLI can't run in, greyed out. A sheet
 * shows no line under each mode, so its message says what Auto runs.
 */
export const permissionSheet = (
	view: ViewState,
	agent: Agent,
	permission: AgentPermission,
	pick: (mode: PermissionMode) => void
): Sheet => {
	const running = runningMode(permission);
	const acp = agent.account ? view.tables.account.get(agent.account)?.acp : undefined;
	const modes = permissionChoices(agent.vendor, acp);
	const runs =
		permissionNote(permission) ?? t("Runs as {{mode}}.", { mode: permissionLabel(running) });
	const auto = modes.some((m) => m.mode === "auto" && m.offered);
	return {
		title: t("Permission mode"),
		message: auto ? `${runs} ${t(AUTO_NOTE)}` : runs,
		choices: modes.map(({ mode, label, offered }) => ({
			label: offered ? label : t("{{label}} ({{reason}})", { label, reason: t(NOT_OFFERED) }),
			disabled: !offered || mode === running,
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
			? t("Runs {{model}} on {{account}}.", { model: runs, account: accountName(account) })
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
				label: t("Move to {{account}}", { account: accountName(move.account) }),
				pick: () => open(moveSheet(move, pick)),
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
	move: Move,
	pick: (model: string, account?: AccountId) => void
): Sheet => ({
	title: t("Move to {{account}}", { account: accountName(move.account) }),
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
