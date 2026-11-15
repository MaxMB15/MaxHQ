// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a running agent's controls offer: the permission modes it can run
// in, the models it can switch to, the accounts it can move to, and
// whether MaxHQ can stop it. The desktop's pickers and header menu and the
// phone's More sheet draw from these, so they offer the same things.

import { t } from "../i18n";

import type {
	AcpFeatures,
	Agent,
	AgentPermission,
	ModelChoice,
	PermissionMode,
	Vendor,
} from "../bindings";
import { AUTO_RUNS } from "../settings/auto-mode";
import { MODE_HINT, MODE_LABEL, MODES, modeShown, vendorHasMode } from "../settings/permission";
import type { Move } from "../view/select";

/** One permission mode as a picker lists it. */
export type PermissionEntry = {
	readonly mode: PermissionMode;
	readonly label: string;
	readonly detail: string;
};

/** What each permission mode is called and what it does, in menu order. */
export const PERMISSION_MODES: readonly PermissionEntry[] = MODES.map((mode) => ({
	mode,
	get label() {
		return MODE_LABEL[mode];
	},
	// A picker has less room, so Auto says what it runs unasked.
	get detail() {
		return mode === "auto" ? t(AUTO_RUNS) : MODE_HINT[mode];
	},
}));

/** A mode's name. */
export const permissionLabel = (mode: PermissionMode): string => MODE_LABEL[mode] ?? mode;

/** The mode the agent runs in, which is the one the CLI reports once it has. */
export const runningMode = (permission: AgentPermission): PermissionMode =>
	permission.effective ?? permission.requested;

/** One mode in the picker, and whether the vendor's CLI can run in it. */
export type PermissionChoice = PermissionEntry & { readonly offered: boolean };

/**
 * The modes the picker lists for the vendor. `acp` is what a local ACP
 * agent can do, which hides the modes it can't.
 */
export const permissionChoices = (vendor: Vendor, acp?: AcpFeatures | null): PermissionChoice[] =>
	PERMISSION_MODES.filter(({ mode }) => modeShown(vendor, mode, acp)).map((entry) => ({
		...entry,
		offered: vendorHasMode(vendor, entry.mode, acp),
	}));

/**
 * What a mode the vendor can't run in says instead of its detail. Plan for
 * Codex is the one such mode, since the others are hidden.
 */
export const NOT_OFFERED = "Not in Codex";

/** Says so when the CLI runs in another mode than the one you asked for. */
export const permissionNote = (permission: AgentPermission): string | null =>
	permission.effective !== null && permission.effective !== permission.requested
		? t(
				"Runs as {{permissionLabel_permission_effective}}. You asked for {{permissionLabel_permission_requested}}.",
				{
					permissionLabel_permission_effective: permissionLabel(permission.effective),
					permissionLabel_permission_requested: permissionLabel(permission.requested),
				}
			)
		: null;

// A Claude model's id, such as "claude-opus-5-5", "claude-sonnet-4-5-20250929"
// or "claude-3-5-haiku-20241022": its family, its version before or after
// that, a date, and "[1m]" for the long context window.
const CLAUDE_MODEL =
	/^claude-(?:(\d{1,2}(?:-\d)?)-)?([a-z]+)(?:-(\d{1,2}(?:-\d)?))?(?:-\d{8})?(\[1m\])?$/;

/**
 * A Claude model's name as Claude Code's model list gives it, such as
 * "Opus 5.5" for "claude-opus-5-5", or null for any other id. A Claude
 * agent reports the model it runs by its id, while the list names the
 * aliases you pick, such as "opus".
 */
export const modelName = (model: string): string | null => {
	const parts = CLAUDE_MODEL.exec(model);
	if (!parts) return null;
	const [, before, family = "", after, wide] = parts;
	const version = (after ?? before)?.replace("-", ".");
	const name = family.charAt(0).toUpperCase() + family.slice(1);
	return [name, version, wide && "(1M)"].filter(Boolean).join(" ");
};

/** The entry in `choices` for the agent's model, else the account's default. */
const choiceOf = (model: string | null, choices: readonly ModelChoice[]) => {
	if (model === null) return choices.find((entry) => entry.default);
	const name = modelName(model);
	return (
		choices.find((entry) => entry.id === model) ??
		choices.find((entry) => name !== null && entry.label === name)
	);
};

/**
 * What the model picker shows for the agent's model: its name in the
 * account's list, else its own name, else its id.
 */
export const modelLabel = (model: string | null, choices: readonly ModelChoice[]): string =>
	choiceOf(model, choices)?.label ??
	(model === null ? t("Default model") : (modelName(model) ?? model));

/** The model the picker marks: the agent's own, else the account's default. */
export const currentModel = (model: string | null, choices: readonly ModelChoice[]): string =>
	choiceOf(model, choices)?.id ?? model ?? "";

/**
 * The name of the agent's model when it can't read images, else null.
 * `maxd` sends such a model each image by its file path.
 */
export const modelWithoutImages = (
	model: string | null,
	choices: readonly ModelChoice[]
): string | null => {
	const choice = choiceOf(model, choices);
	return choice?.no_images ? choice.label : null;
};

/** The models a move offers. Without a list, the account's own default runs. */
export const moveModels = (move: Move): readonly Pick<ModelChoice, "id" | "label">[] =>
	move.models.length > 0 ? move.models : [{ id: "", label: t("Default model") }];

/**
 * Whether you can pick the agent's model, with `models` and `moves` from
 * `modelsOf` and `movesOf`. A sub-agent runs on the model its lead picked
 * for it.
 */
export const picksModel = (
	agent: Agent,
	models: readonly ModelChoice[],
	moves: readonly Move[]
): boolean =>
	agent.control !== "observed" &&
	agent.parent === null &&
	(models.length > 0 || moves.length > 0);

/** Whether MaxHQ can end the agent's process. */
export const canEnd = (agent: Agent): boolean =>
	agent.control === "held" && agent.session !== null && agent.link.state !== "down";

/** What Stop agent is called and what it does. */
export const STOP_AGENT = {
	get label() {
		return t("Stop agent");
	},
	get detail() {
		return t("Ends its process. The conversation stays.");
	},
} as const;
