// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether a project's Workflows entry is folded. It starts folded once the
// project has more than a few runs, and stays the way you last left it.

import type { ProjectId } from "../bindings";

/** A Workflows entry with more runs than this starts folded. */
export const FOLD_ABOVE = 5;

const foldKey = (project: ProjectId) => `workflows:${project}`;
const openKey = (project: ProjectId) => `workflows-open:${project}`;

/** Whether a project's Workflows entry is folded, given the folds you made. */
export const workflowsFolded = (
	collapsed: ReadonlySet<string>,
	project: ProjectId,
	runs: number
): boolean => {
	if (collapsed.has(foldKey(project))) return true;
	if (collapsed.has(openKey(project))) return false;
	return runs > FOLD_ABOVE;
};

/** The folds with these projects' Workflows entries folded or opened. */
export const foldWorkflows = (
	collapsed: ReadonlySet<string>,
	projects: readonly ProjectId[],
	folded: boolean
): ReadonlySet<string> => {
	const next = new Set(collapsed);
	for (const project of projects) {
		next.delete(foldKey(project));
		next.delete(openKey(project));
		next.add(folded ? foldKey(project) : openKey(project));
	}
	return next;
};
