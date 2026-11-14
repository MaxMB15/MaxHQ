// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the `@` and `#` menus find in the sample build: a small workspace
// and a repository's pull requests and issues, matched the way the backend
// matches them.

import type { Timestamp } from "../bindings";
import type { GitItemRef, GitItemSearch, PathMatch } from "./source";

const PATHS: readonly PathMatch[] = [
	{ path: "src", kind: "folder" },
	{ path: "src/main.rs", kind: "file" },
	{ path: "src/cache.rs", kind: "file" },
	{ path: "src/usage", kind: "folder" },
	{ path: "src/usage/meter.rs", kind: "file" },
	{ path: "src/usage/poll.rs", kind: "file" },
	{ path: "tests", kind: "folder" },
	{ path: "tests/cache.rs", kind: "file" },
	{ path: "docs", kind: "folder" },
	{ path: "docs/setup.md", kind: "file" },
	{ path: "Cargo.toml", kind: "file" },
	{ path: "README.md", kind: "file" },
];

/** At most this many paths come back, like the backend's search. */
const MAX_PATHS = 50;

/** Whether each character of the query shows up in the path, in order. */
const fuzzy = (path: string, query: string): boolean => {
	let at = 0;
	for (const char of path.toLowerCase()) if (char === query[at]) at += 1;
	return at === query.length;
};

/** Files and folders whose path holds the query's characters in order. */
export const findMockPaths = (query: string): PathMatch[] => {
	const wanted = query.toLowerCase();
	return PATHS.filter((match) => fuzzy(match.path, wanted)).slice(0, MAX_PATHS);
};

type Seed = Pick<GitItemRef, "number" | "kind" | "title" | "state" | "head" | "base" | "body">;

const SEEDS: readonly Seed[] = [
	{
		number: 412,
		kind: "pull_request",
		title: "Cache usage between polls",
		state: "open",
		head: "usage-cache",
		base: "main",
		body: "Keeps the last answer for 30 seconds, so the meter stops asking twice.",
	},
	{
		number: 409,
		kind: "pull_request",
		title: "Sort the sidebar by last activity",
		state: "draft",
		head: "sidebar-sort",
		base: "main",
		body: "Still needs the pinned agents to stay on top.",
	},
	{
		number: 405,
		kind: "issue",
		title: "Usage meter shows the wrong reset time",
		state: "open",
		head: null,
		base: null,
		body: "The weekly window resets on Monday, but the meter says Sunday.",
	},
	{
		number: 398,
		kind: "pull_request",
		title: "Check the bindings in CI",
		state: "merged",
		head: "bindings-check",
		base: "main",
		body: "Fails the build when the generated bindings are out of date.",
	},
	{
		number: 391,
		kind: "issue",
		title: "Release checklist for M1",
		state: "completed",
		head: null,
		base: null,
		body: "Everything that has to pass before the M1 build ships.",
	},
	{
		number: 377,
		kind: "pull_request",
		title: "Try a second cache layer",
		state: "closed",
		head: "cache-layer",
		base: "main",
		body: "Closed in favor of the simpler cache.",
	},
	{
		number: 360,
		kind: "issue",
		title: "Dark mode for the login sheet",
		state: "not_planned",
		head: null,
		base: null,
		body: "The login sheet follows the system for now.",
	},
];

const MAX_ITEMS = 20;

/**
 * Pull requests and issues in `repo` that match the query. Digits find that
 * number first, and words search the titles.
 */
export const findMockGitItems = (
	repo: string | null,
	query: string,
	now: Timestamp
): GitItemSearch => {
	if (repo === null)
		return {
			repo: null,
			items: [],
			unavailable: "This agent's folder has no GitHub repository",
		};
	const items = SEEDS.map((seed): GitItemRef => ({
		type: "git_item",
		repo,
		url: `https://github.com/${repo}/${seed.kind === "pull_request" ? "pull" : "issues"}/${seed.number}`,
		checked_at: now,
		...seed,
	}));
	const words = query.toLowerCase().split(/\s+/).filter(Boolean);
	const found = /^\d+$/.test(query)
		? items.filter((item) => String(item.number).includes(query))
		: items.filter((item) => words.every((word) => item.title.toLowerCase().includes(word)));
	return { repo, items: found.slice(0, MAX_ITEMS), unavailable: null };
};
