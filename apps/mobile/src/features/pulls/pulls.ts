// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the phone's PR screens pick: the projects the Pulls screen offers,
// those whose device holds a GitHub App reaching their repository, by
// name, the device an agent's PRs are read on, what a PR chip's menu
// offers, in the desktop's order, and what Merge asks first.

import { formatNumber, t } from "@maxhq/shared/i18n";

import type {
	Agent,
	DeviceId,
	MergeMethod,
	Project,
	PullDetail,
	PullKey,
	PullLink,
	Timestamp,
} from "@maxhq/shared/bindings";
import { appReaches } from "@maxhq/shared/pulls/app";
import { type PullPrompt, pullPrompts } from "@maxhq/shared/pulls/chip";
import { MERGE_METHODS } from "@maxhq/shared/pulls/merge";
import { all, type ViewState } from "@maxhq/shared/view/replica";

/** The projects with a Pulls page, as the desktop's project menu offers them, by name. */
export const pullProjects = (view: ViewState, now: Timestamp): Project[] =>
	all(view, "project")
		.filter((project) => appReaches(view, project, now))
		.sort((a, b) => a.name.localeCompare(b.name));

/** The device an agent runs on, which reads its PRs. */
export const agentDevice = (
	view: ViewState,
	agent: Pick<Agent, "workspace">
): DeviceId | undefined => view.tables.workspace.get(agent.workspace)?.device;

/** A PR as one string, for list keys and reads. Repositories compare in lower case. */
export const pullId = (key: PullKey): string => `${key.repo.toLowerCase()}#${key.number}`;

/** One thing a PR chip's menu does. */
export type ChipAction =
	| { readonly kind: "github" }
	| { readonly kind: "prompt"; readonly prompt: PullPrompt }
	| { readonly kind: "unlink" }
	| { readonly kind: "link" };

/**
 * What a PR chip's menu offers: the PR on GitHub, the messages about it,
 * unlinking it and linking another. Only an agent MaxHQ holds takes
 * messages.
 */
export const chipActions = (agent: Pick<Agent, "control">, link: PullLink): ChipAction[] => [
	{ kind: "github" },
	...(agent.control === "observed" ? [] : pullPrompts(link)).map((prompt) => ({
		kind: "prompt" as const,
		prompt,
	})),
	{ kind: "unlink" },
	{ kind: "link" },
];

/**
 * What Merge asks first on the phone, which gets more stray taps than a
 * mouse: the PR, where it goes and how, since a merge can't be undone.
 * Its button is the method, as the Merge button is.
 */
export const mergeQuestion = (
	pr: PullDetail,
	method: MergeMethod
): { readonly title: string; readonly detail: string; readonly action: string } => {
	const { key, pull } = pr.summary;
	const action = MERGE_METHODS[method].title;
	return {
		title: t("Merge pull request #{{number}}?", {
			number: formatNumber(key.number, { useGrouping: false }),
		}),
		detail: t("{{method}}: {{title}}, into {{base}} in {{repo}}. You can't undo a merge.", {
			method: action,
			title: pull.title,
			base: pull.base,
			repo: key.repo,
		}),
		action,
	};
};
