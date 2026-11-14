// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The GitHub App a device reads PRs through: whether it reaches a
// project's repository, which decides whether the project gets a Pulls
// entry, and where you accept the App's new permissions when its checks
// read as unknown.

import type { CredentialKind, DeviceId, Project, Timestamp } from "../bindings";
import { all, type ViewState } from "../view/replica";
import { pullsDevice } from "./devices";
import { sameRepo } from "./links";

/** A GitHub App credential. */
export type GithubApp = Extract<CredentialKind, { type: "github_app" }>;

/**
 * A clone URL on GitHub, such as `https://github.com/owner/name.git`,
 * `git@github.com:owner/name` or `github.com/owner/name`, or a bare
 * "owner/name".
 */
const CLONE =
	/^(?:(?:(?:https?|ssh):\/\/)?(?:git@)?github\.com[/:])?([\w-]+)\/([\w.-]+?)(?:\.git)?\/?$/u;

/** A project's repository as "owner/name", or null when it isn't on GitHub. */
export const projectRepo = (project: Project): string | null => {
	const match = project.repo ? CLONE.exec(project.repo.trim()) : null;
	return match ? `${match[1]}/${match[2]}` : null;
};

/** The GitHub App a device holds, if it holds one. */
export const githubAppOn = (view: ViewState, device: DeviceId): GithubApp | undefined => {
	for (const credential of all(view, "credential")) {
		if (credential.device === device && credential.kind.type === "github_app") {
			return credential.kind;
		}
	}
	return undefined;
};

/** Whether the device that reads a project's PRs holds an App reaching its repository. */
export const appReaches = (view: ViewState, project: Project, now: Timestamp): boolean => {
	const repo = projectRepo(project);
	const device = pullsDevice(view, project.id, now);
	const app = device ? githubAppOn(view, device) : undefined;
	return repo !== null && app !== undefined && app.repos.some((one) => sameRepo(one, repo));
};

/** Where you accept an App's new permissions, or null when GitHub hasn't said its slug. */
export const appInstallUrl = (app: GithubApp | undefined): string | null =>
	app?.slug ? `https://github.com/apps/${app.slug}/installations/new` : null;
