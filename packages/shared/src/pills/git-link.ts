// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A pasted link to a pull request or issue on GitHub. A paste that's one
// such link and nothing else becomes the item's pill once the lookup finds
// it, the way `#` makes one. Any other paste stays text.

import type { GitTarget } from "./draft";

/** A link to a pull request or issue, with its repo. */
export type GitLink = GitTarget & { readonly repo: string };

/**
 * `https://github.com/owner/repo/pull/12` or `.../issues/12`. A pull
 * request's link may go on to its files, commits or checks, and either may
 * end in a slash, a query or a fragment. The number has at most nine
 * digits, the most a `#123` takes.
 */
const LINK =
	/^https:\/\/github\.com\/([\w.-]+)\/([\w.-]+)\/(pull|issues)\/([1-9]\d{0,8})(\/(?:files|commits|checks))?\/?(?:\?[^\s#]*)?(?:#\S*)?$/u;

/** A name that's only dots, which GitHub doesn't give an owner or a repo. */
const DOTS = /^\.+$/u;

/** The pull request or issue a paste links to, when the paste is that link alone. */
export const gitLinkOf = (text: string): GitLink | null => {
	const match = LINK.exec(text.trim());
	if (!match) return null;
	const [, owner = "", name = "", kind, number = "", more] = match;
	if (DOTS.test(owner) || DOTS.test(name)) return null;
	if (more && kind !== "pull") return null;
	return { repo: `${owner}/${name}`, number: Number(number) };
};

/** Whether two "owner/name" repos are the same, as GitHub reads names, whatever their case. */
export const sameRepo = (a: string, b: string): boolean => a.toLowerCase() === b.toLowerCase();
