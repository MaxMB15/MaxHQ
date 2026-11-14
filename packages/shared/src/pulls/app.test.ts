// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import type { Entity, Project } from "../bindings";
import { device, entities, link, NOW, project, workspace } from "../test/build";
import { credential } from "../test/pulls";
import { applySnapshot } from "../view/replica";

import { appInstallUrl, appReaches, type GithubApp, githubAppOn, projectRepo } from "./app";

const withRepo = (repo: string | null): Project => ({ ...project("p", "P"), repo });

const APP: GithubApp = {
	type: "github_app",
	app_id: 1,
	slug: "my-app",
	key_sha256: "00",
	repos: ["Owner/Repo"],
};

const viewWith = (repo: string | null, extra: Entity[] = []) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				devices: [device({ id: "dev-box", name: "box" })],
				links: [
					link({
						id: "box",
						name: "box",
						capabilities: {
							transfers: true,
							tunnels: true,
							workspaces: true,
							pulls: true,
						},
					}),
				],
				projects: [withRepo(repo)],
				workspaces: [workspace({ id: "w", project: "p", device: "dev-box" })],
			}),
			...extra,
		],
	});

describe("projectRepo", () => {
	it("reads the repository from a GitHub clone URL", () => {
		for (const url of [
			"https://github.com/owner/repo.git",
			"git@github.com:owner/repo.git",
			"ssh://git@github.com/owner/repo",
			"github.com/owner/repo",
			"owner/repo",
			"https://github.com/owner/repo/",
		]) {
			expect(projectRepo(withRepo(url))).toBe("owner/repo");
		}
		expect(projectRepo(withRepo("https://github.com/owner/my.repo"))).toBe("owner/my.repo");
	});

	it("is null off GitHub or without a repository", () => {
		expect(projectRepo(withRepo(null))).toBeNull();
		expect(projectRepo(withRepo("https://gitlab.com/owner/repo"))).toBeNull();
		expect(projectRepo(withRepo("https://github.com/owner"))).toBeNull();
	});
});

describe("appReaches", () => {
	it("is true when the project's device holds an App on its repository", () => {
		const view = viewWith("github.com/owner/repo", [credential("dev-box", APP)]);
		expect(githubAppOn(view, "dev-box")).toEqual(APP);
		expect(appReaches(view, view.tables.project.get("p")!, NOW)).toBe(true);
	});

	it("is false for another repository, or with no App", () => {
		const other = viewWith("github.com/owner/other", [credential("dev-box", APP)]);
		expect(appReaches(other, other.tables.project.get("p")!, NOW)).toBe(false);
		const none = viewWith("github.com/owner/repo");
		expect(githubAppOn(none, "dev-box")).toBeUndefined();
		expect(appReaches(none, none.tables.project.get("p")!, NOW)).toBe(false);
	});
});

describe("appInstallUrl", () => {
	it("links to the App's installation page once GitHub has said its slug", () => {
		expect(appInstallUrl(APP)).toBe("https://github.com/apps/my-app/installations/new");
		expect(appInstallUrl({ ...APP, slug: null })).toBeNull();
		expect(appInstallUrl(undefined)).toBeNull();
	});
});
