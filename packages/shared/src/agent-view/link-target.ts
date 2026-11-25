// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where a link in a reply goes. A web link opens in the browser, a link
// within the message moves to its target there, and a file link opens
// the file in MaxHQ's viewer, at its line when the link names one. Agents
// write a file's line as `file.rs:12`, `file.rs:12:5` or `file.rs#L12`.
// A path goes to the agent's device as written, and maxd reads a relative
// one from the agent's folder. Anything else can't open, and shows as
// plain text.

import {
	fileUrlPath,
	isNetworkPath,
	normalizeDestination,
	safeDecode,
	splitSearchAndHash,
	stripSlashDrive,
} from "./media-source";
import { isAbsolutePath, namesOf, rootOf, separatorOf } from "../folders/path";

/** Where a link goes. */
export type LinkTarget =
	| { readonly kind: "web"; readonly url: string }
	| { readonly kind: "anchor"; readonly id: string }
	| { readonly kind: "file"; readonly path: string; readonly line: number | null }
	| { readonly kind: "none" };

const NONE: LinkTarget = { kind: "none" };
const WEB = /^(?:https?:\/\/|mailto:)/i;
const SCHEME = /^[A-Za-z][A-Za-z0-9+.-]*:/;
const POSITION = /:(\d+)(?::\d+)?$/;
// `#L12`, `#L12C5`, `#L12-L20` and `#L12-20`, as code hosts write them.
const LINE_HASH = /^#L(\d+)(?:C\d+)?(?:-L?\d+(?:C\d+)?)?$/i;

const lineOf = (digits: string | undefined): number | null => {
	const line = digits === undefined ? NaN : Number(digits);
	return Number.isSafeInteger(line) && line > 0 ? line : null;
};

/** A path and the line written after it, as `:12` or `:12:5`. */
const splitPosition = (path: string): { path: string; line: number | null } => {
	const match = POSITION.exec(path);
	if (!match || match.index === 0) return { path, line: null };
	return { path: path.slice(0, match.index), line: lineOf(match[1]) };
};

const fileAt = (path: string, hash: string): LinkTarget => {
	if (path.length === 0 || isNetworkPath(path)) return NONE;
	const split = splitPosition(path);
	if (split.path.length === 0) return NONE;
	const line = split.line ?? lineOf(LINE_HASH.exec(hash)?.[1]);
	return { kind: "file", path: split.path, line };
};

/** Where a link's target goes, from the href as the reply wrote it. */
export const linkTarget = (href: string | null | undefined): LinkTarget => {
	if (href === null || href === undefined) return NONE;
	const target = normalizeDestination(href);
	if (target.length === 0) return NONE;
	if (target.startsWith("#")) {
		const id = safeDecode(target.slice(1));
		return id.length > 0 ? { kind: "anchor", id } : NONE;
	}
	if (WEB.test(target)) return { kind: "web", url: target };
	if (target.startsWith("//")) return { kind: "web", url: `https:${target}` };
	const { hash } = splitSearchAndHash(target);
	if (/^file:/i.test(target)) {
		const path = fileUrlPath(target);
		return path === null ? NONE : fileAt(path, hash);
	}
	const path = stripSlashDrive(safeDecode(splitSearchAndHash(target).path));
	if (isAbsolutePath(path)) return fileAt(path, hash);
	if (path.startsWith("~")) return NONE;
	// `windows.rs:12` looks like a scheme, so a link counts as one only
	// when it's still one without its line.
	if (SCHEME.test(splitPosition(path).path)) return NONE;
	return fileAt(path, hash);
};

/**
 * A relative path read from `folder`, as a link in a Markdown file means
 * it, with the folder's separator. An absolute path, or one with no folder
 * to read it from, stays. A `..` past a root such as `/` or `C:\` stops there.
 */
export const pathFrom = (folder: string, path: string): string => {
	if (folder.length === 0 || isAbsolutePath(path)) return path;
	const root = rootOf(folder);
	const names = namesOf(folder);
	for (const part of path.split(/[/\\]/)) {
		if (part === "" || part === ".") continue;
		if (part === ".." && names.length > 0 && names.at(-1) !== "..") names.pop();
		else if (part !== ".." || root === "") names.push(part);
	}
	return root + names.join(separatorOf(folder));
};

/**
 * The id a heading takes from its text, as code hosts make them, so
 * `[Setup](#setup-steps)` finds "Setup steps".
 */
export const headingSlug = (text: string): string =>
	text
		.trim()
		.toLowerCase()
		.replace(/[^\p{L}\p{N}\s_-]/gu, "")
		.replace(/\s/g, "-");
