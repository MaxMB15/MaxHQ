// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The New agent screen's own logic, apart from what it draws: the choices
// it opens with, the effort levels it offers for the login and model you
// picked, the level that goes in the spec, and the branches it suggests
// under the branch field. Effort shows only for Claude Code and Codex on a
// maxd that sets levels, and Default leaves the level to the device.

import { effortChoices, setsEffort, takesEffort } from "@maxhq/shared/agent-view/effort";
import type { AgentSpec, ProjectId } from "@maxhq/shared/bindings";
import { t } from "@maxhq/shared/i18n";
import { type BranchSuggestion, filterBranches } from "@maxhq/shared/new-agent/branch";
import {
	type Choices,
	emptyChoices,
	type Option,
	projectOptions,
	type Resolved,
} from "@maxhq/shared/new-agent/spec";
import type { ViewState } from "@maxhq/shared/view/replica";

/** The choices the form opens with: in `project`, or in a folder when there's no project at all. */
export const initialChoices = (view: ViewState, project: ProjectId | null): Choices => {
	const empty = emptyChoices(project);
	return projectOptions(view).length > 0 ? empty : { ...empty, place: "folder" };
};

/** The effort picker's entry that leaves the level to the device. */
export const DEFAULT_EFFORT = "";

/**
 * The effort picker's entries: Default, then each level the picked login's
 * model takes. None when the login's vendor takes no level, its device's
 * maxd can't set one, or the model lists none, and the picker then hides.
 */
export const effortOptions = (view: ViewState, r: Resolved): Option[] => {
	const { account, device } = r;
	if (!account || !device || !takesEffort(account.vendor) || !setsEffort(view, device)) return [];
	const levels = effortChoices(null, r.model, account.models);
	if (levels.length === 0) return [];
	return [
		{ value: DEFAULT_EFFORT, label: t("Default") },
		...levels.map((level) => ({ value: level.value, label: level.label })),
	];
};

/**
 * The level to send: the one you picked while the picker still offers it,
 * else null for the device's default. A login or model you change to that
 * lacks your level drops it.
 */
export const effortToSend = (options: readonly Option[], picked: string | null): string | null =>
	picked !== null && picked !== DEFAULT_EFFORT && options.some((o) => o.value === picked)
		? picked
		: null;

/** The spec with the level you picked. With none, the spec leaves it out. */
export const withEffort = (spec: AgentSpec, effort: string | null): AgentSpec =>
	effort === null ? spec : { ...spec, effort };

/** How many branches the field suggests under it at once. */
export const SHOWN_BRANCHES = 6;

/**
 * The branches to suggest under the field: the first few that match what's
 * typed, leaving out the one already typed whole.
 */
export const shownBranches = (
	suggestions: readonly BranchSuggestion[],
	typed: string
): BranchSuggestion[] => {
	const name = typed.trim();
	return filterBranches(suggestions, typed)
		.filter((s) => s.name !== name)
		.slice(0, SHOWN_BRANCHES);
};
