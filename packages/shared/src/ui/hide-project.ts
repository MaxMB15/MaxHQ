// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Hiding a project from the agent list, or showing it again. A hidden
// project's group shows only while one of its agents calls it back, and
// the Hidden projects entry lists each one with a way to show it again.
// What goes wrong shows in the list's notice, on every client.

import { t } from "../i18n";

import type { Project } from "../bindings";
import { failureOf } from "../data/failure";
import { sentence } from "../status/format";

import { type InboxNotices, showFailure } from "./inbox-notice";

/** What the notice says when hiding or showing `project` didn't go through. */
export const hideFailure = (project: Project, hidden: boolean, failure: string): string => {
	const tried = t("Couldn't {{value1}} {{name}}", {
		value1: hidden ? "hide" : "show",
		name: project.name,
	});
	return `${tried}. ${sentence(failure)}.`;
};

/** Hides the project on every device, or shows it again, saying what went wrong. */
export const setProjectHidden = async (
	notices: InboxNotices,
	project: Project,
	hidden: boolean
): Promise<void> => {
	const failure = failureOf(await notices.source.setProjectHidden(project.id, hidden));
	if (failure !== null) showFailure(notices, hideFailure(project, hidden, failure));
};
