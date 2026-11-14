// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The branch an agent in a container works on: whether the guest takes the
// name, the branches the field suggests as you type, and the agents that
// already work on one. The name rules are `check_name` in
// crates/daemon/src/guest/worktree.rs, and both sides test the names in
// crates/daemon/fixtures/branch-names.json so they can't drift. The
// suggestions come from what the client already has, the agents' branches
// and the project's open PRs, and from the repo's branches when `maxd`
// could list them, and they match what you type the way a branch
// picker does.

import { t } from "../i18n";

import type { Agent, ProjectId, PullSummary, WorkspaceId } from "../bindings";
import { projectRepo } from "../pulls/app";
import { sameRepo } from "../pulls/links";
import { all, type ViewState } from "../view/replica";

/** What the field says under itself when another agent has the branch. */
export const SHARED_BRANCH_NOTE = "Another agent works on this branch. They'll share its folder.";

// The characters a branch can have, the guest's `fine`.
const FINE = /^[A-Za-z0-9._/-]$/;

/** Why the guest would refuse `name` as a branch, or null when it takes it. */
export const branchProblem = (name: string): string | null => {
	if (name === "") return t("Give the branch a name.");
	if (name.startsWith("-")) return t("A branch can't start with a dash.");
	if (name.startsWith("/")) return t("A branch can't start with a slash.");
	if (name.endsWith("/")) return t("A branch can't end with a slash.");
	if (name.endsWith(".lock")) return t("A branch can't end with .lock.");
	if (name.endsWith(".")) return t("A branch can't end with a dot.");
	if (name.includes("..")) return t("A branch can't have two dots in a row.");
	if (name.includes("//")) return t("A branch can't have two slashes in a row.");
	if (name.includes("@{")) return t('A branch can\'t have "@{" in it.');
	const odd = [...name].find((c) => !FINE.test(c));
	if (odd === undefined) return null;
	if (/\s/.test(odd)) return t("A branch can't have spaces.");
	return t('A branch can\'t have "{{odd}}" in it.', { odd: odd });
};

/** A branch the field suggests. */
export type BranchSuggestion = {
	readonly name: string;
	/** The open PR it's the head of, if any. */
	readonly pull: number | null;
	/** Whether an agent in the project works on it now. */
	readonly current: boolean;
};

/** The repo's branches as `maxd` listed them. */
export type RepoBranches = {
	readonly branches: readonly string[];
	readonly default_branch: string | null;
};

// A head that an open PR has, and when the PR last changed.
type Head = { readonly name: string; readonly pull: number; readonly at: number };

const agentsOf = (view: ViewState, project: ProjectId): Agent[] =>
	all(view, "agent").filter(
		(a) => a.parent === null && view.tables.workspace.get(a.workspace)?.project === project
	);

// The open PRs' heads, from the list `maxd` read and the PRs the project's
// agents link to, newest change first.
const openHeads = (view: ViewState, project: ProjectId, pulls: readonly PullSummary[]): Head[] => {
	const known = view.tables.project.get(project);
	const repo = known ? projectRepo(known) : null;
	// A project whose repo isn't known keeps its agents' PRs.
	const ours = (other: string) => repo === null || sameRepo(other, repo);
	const linked = agentsOf(view, project).flatMap((a) =>
		(a.pulls ?? []).flatMap(({ key, snapshot: s }) =>
			s?.state === "open" && ours(key.repo)
				? [{ name: s.head, pull: key.number, at: s.updated_at }]
				: []
		)
	);
	const listed = pulls
		.filter((p) => p.pull.state === "open")
		.map((p) => ({ name: p.pull.head, pull: p.key.number, at: p.pull.updated_at }));
	return [...listed, ...linked].sort((a, b) => b.at - a.at);
};

/**
 * Every branch the field can suggest for a project: the branches its agents
 * work on now, newest first, then its open PRs' heads, then the rest of the
 * repo's branches. Each name shows once. The repo's default branch is left
 * out, since its checkout already has it, and so is any name the guest
 * would refuse.
 */
export const branchSuggestions = (
	view: ViewState,
	project: ProjectId,
	pulls: readonly PullSummary[],
	repo: RepoBranches | undefined
): BranchSuggestion[] => {
	const current = agentsOf(view, project)
		.filter((a) => a.branch !== null)
		.sort((a, b) => b.last_activity_at - a.last_activity_at)
		.map((a) => a.branch!);
	const heads = openHeads(view, project, pulls);
	const pullOf = new Map<string, number>();
	for (const head of heads) if (!pullOf.has(head.name)) pullOf.set(head.name, head.pull);
	const working = new Set(current);
	const names = new Set([...current, ...heads.map((h) => h.name), ...(repo?.branches ?? [])]);
	return [...names]
		.filter((name) => name !== repo?.default_branch && branchProblem(name) === null)
		.map((name) => ({ name, pull: pullOf.get(name) ?? null, current: working.has(name) }));
};

/**
 * The suggestions that match what's typed: the ones that have it in their
 * name, ignoring case, or that have it with its spaces turned into dashes,
 * as a branch would. Nothing typed matches them all.
 */
export const filterBranches = (
	suggestions: readonly BranchSuggestion[],
	typed: string
): BranchSuggestion[] => {
	const query = typed.trim().toLowerCase();
	if (query === "") return [...suggestions];
	const dashed = query.replace(/\s+/g, "-");
	return suggestions.filter((s) => {
		const name = s.name.toLowerCase();
		return name.includes(query) || name.includes(dashed);
	});
};

/** The agents in a workspace that work on `branch`, which share its folder. */
export const agentsOnBranch = (view: ViewState, workspace: WorkspaceId, branch: string): Agent[] =>
	all(view, "agent").filter(
		(a) => a.parent === null && a.workspace === workspace && a.branch === branch
	);

// The folder the guest puts a branch's worktree in, as `path` in
// crates/daemon/src/guest/worktree.rs names it.
const branchFolder = (branch: string): string => branch.replace(/\//g, "-");

/**
 * Why `branch` can't go in the workspace because another agent's branch
 * already has its folder, as `a/b` and `a-b` would, or null. The guest
 * refuses it at Start, so the form says so first.
 */
export const folderClash = (
	view: ViewState,
	workspace: WorkspaceId,
	branch: string
): string | null => {
	const folder = branchFolder(branch);
	const other = all(view, "agent").find(
		(a) =>
			a.parent === null &&
			a.workspace === workspace &&
			a.branch !== null &&
			a.branch !== branch &&
			branchFolder(a.branch) === folder
	);
	return other
		? t("{{other_branch}} already has this branch's folder. Pick another name.", {
				other_branch: other.branch,
			})
		: null;
};

/** The tag on a suggestion: its PR, or that an agent works on it. */
export const branchTag = (suggestion: BranchSuggestion): string | null => {
	if (suggestion.pull !== null) return t("PR #{{value1}}", { value1: String(suggestion.pull) });
	return suggestion.current ? t("In use") : null;
};
