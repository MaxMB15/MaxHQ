// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a path field makes of what you type: the folder to list and the
// name to look for in it, `~` read with the device's home folder, the
// entries that fit the name and what a pick puts in the field. Paths here
// are a Mac's or Linux's, with `/` between names.

import { t } from "../i18n";

import type { FolderEntry } from "../bindings";

/** Whether a field wants a folder or a file. */
export type PathWants = "folder" | "file";

/** The most suggestions a field shows at once. */
export const MAX_SUGGESTIONS = 8;

/** Whether `text` reads as a path: a full one, or one from home. */
export const isPathText = (text: string): boolean =>
	text.startsWith("/") || text === "~" || text.startsWith("~/");

/**
 * `text` with a leading `~` read as `home`. Anything else, or any text
 * while the home folder isn't known, comes back as it is.
 */
export const expandHome = (text: string, home: string | null): string => {
	if (home === null || !(text === "~" || text.startsWith("~/"))) return text;
	return home === "/" && text !== "~" ? text.slice(1) : home + text.slice(1);
};

/** `path` with the home folder shown as `~`, for a sentence. */
export const contractHome = (path: string, home: string | null): string => {
	if (home === null || home === "/") return path;
	if (path === home) return "~";
	return path.startsWith(`${home}/`) ? `~${path.slice(home.length)}` : path;
};

/** The folder `path` is in, or null for `/` and for a bare name. */
export const parentOf = (path: string): string | null => {
	const trimmed = path.length > 1 ? path.replace(/\/+$/u, "") : path;
	const cut = trimmed.lastIndexOf("/");
	if (trimmed === "/" || cut < 0) return null;
	return cut === 0 ? "/" : trimmed.slice(0, cut);
};

/** The last name in `path`. */
export const baseName = (path: string): string => {
	const trimmed = path.replace(/\/+$/u, "");
	return trimmed.slice(trimmed.lastIndexOf("/") + 1);
};

/** `name` inside `folder`. */
export const joinPath = (folder: string, name: string): string =>
	folder.endsWith("/") ? folder + name : `${folder}/${name}`;

/** What typed text asks the device for. */
export type TypedPath = {
	/** The folder to list, as typed: `~`, `/` or a path without its last slash. */
	readonly folder: string;
	/** What goes before each name in a suggestion, as typed, ending in a slash. */
	readonly shown: string;
	/** The name typed so far after the last slash. */
	readonly name: string;
};

/**
 * Splits typed text into the folder to list and the name to look for in
 * it. Text that isn't a path, such as a name on PATH or `owner/repo`,
 * gets no suggestions.
 */
export const splitTyped = (text: string): TypedPath | null => {
	if (!isPathText(text)) return null;
	if (text === "~") return { folder: "~", shown: "~/", name: "" };
	const cut = text.lastIndexOf("/");
	const shown = text.slice(0, cut + 1);
	const folder = cut === 0 ? "/" : text.slice(0, cut);
	return { folder, shown, name: text.slice(cut + 1) };
};

/**
 * The entries that fit the typed name, as many as a field shows: names
 * that start with it first, then names that hold it, each in the order the
 * device listed them, folders first. Files show only where a file goes,
 * and names that start with a dot only once you type one.
 */
export const fitting = (
	entries: readonly FolderEntry[],
	name: string,
	wants: PathWants
): FolderEntry[] => {
	const typed = name.toLowerCase();
	const shown = entries.filter(
		(entry) =>
			(entry.folder || wants === "file") &&
			(!entry.name.startsWith(".") || typed.startsWith("."))
	);
	const starts = shown.filter((entry) => entry.name.toLowerCase().startsWith(typed));
	const holds = shown.filter(
		(entry) => !starts.includes(entry) && entry.name.toLowerCase().includes(typed)
	);
	return [...starts, ...holds].slice(0, MAX_SUGGESTIONS);
};

/** What a suggestion puts in the field: its path, and a slash after a folder on Tab. */
export const pickText = (typed: TypedPath, entry: FolderEntry, completing: boolean): string =>
	typed.shown + entry.name + (completing && entry.folder ? "/" : "");

/** A checkout's tag, such as "git · main", or null for a folder that isn't one. */
export const checkoutTag = (entry: FolderEntry): string | null => {
	if (!entry.checkout) return null;
	const branch = entry.checkout.branch;
	return branch ? t("git · {{branch}}", { branch }) : "git";
};

/** Why a new folder can't take `name`, or null when it can. */
export const folderNameProblem = (name: string): string | null => {
	const trimmed = name.trim();
	if (!trimmed) return t("Name the new folder.");
	if (trimmed.includes("/")) return t("A folder's name can't have a slash.");
	if (trimmed === "." || trimmed === "..") return t("A folder can't be called . or ..");
	return null;
};
