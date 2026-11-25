// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Paths as a device writes them, whichever device that is: a Mac's or
// Linux's, with `/` between names, or Windows', with `\` or `/`, a drive
// letter such as `C:\` or a share such as `\\server\share`. Each path is
// read by its own text, never by the system the app runs on, since a
// device across the network may be another kind. Also what a path field
// makes of what you type: the folder to list and the name to look for in
// it, `~` read with the device's home folder, the entries that fit the
// name and what a pick puts in the field.

import { t } from "../i18n";

import type { FolderEntry } from "../bindings";

/** Whether a field wants a folder or a file. */
export type PathWants = "folder" | "file";

/** The most suggestions a field shows at once. */
export const MAX_SUGGESTIONS = 8;

/** How a path writes its names: with `/`, or the Windows way. */
export type PathStyle = "posix" | "windows";

const DRIVE = /^[A-Za-z]:(?:[\\/]|$)/u;
const DRIVE_ROOT = /^[A-Za-z]:[\\/]?/u;
const SHARE_ROOT = /^\\\\[^\\/]*(?:[\\/][^\\/]*)?[\\/]?/u;

/**
 * Whether `path` is a Windows one: it starts with a drive letter or a
 * share's two backslashes, or it has a backslash and no slash, as a
 * relative path from a Windows agent does.
 */
export const pathStyleOf = (path: string): PathStyle =>
	DRIVE.test(path) || path.startsWith("\\\\") || (path.includes("\\") && !path.includes("/"))
		? "windows"
		: "posix";

/** Whether `path` starts at a Windows drive or share, such as `C:\` or `\\server\share`. */
export const isWindowsAbsolute = (path: string): boolean =>
	path.startsWith("\\\\") || DRIVE.test(path);

/** Whether `path` is a full one, on any device. */
export const isAbsolutePath = (path: string): boolean =>
	path.startsWith("/") || isWindowsAbsolute(path);

/** The separator to put between names in `path`: `\` on Windows, unless it writes `/`. */
export const separatorOf = (path: string): "/" | "\\" =>
	pathStyleOf(path) === "windows" && (path.includes("\\") || !path.includes("/")) ? "\\" : "/";

const lastSeparator = (text: string, style: PathStyle): number =>
	style === "windows"
		? Math.max(text.lastIndexOf("/"), text.lastIndexOf("\\"))
		: text.lastIndexOf("/");

/**
 * Where `path` starts: `/`, a drive such as `C:\`, a share such as
 * `\\server\share\`, as the path writes it, or "" for a relative path.
 */
export const rootOf = (path: string): string => {
	if (pathStyleOf(path) === "posix") return path.startsWith("/") ? "/" : "";
	return (DRIVE.test(path) ? DRIVE_ROOT : SHARE_ROOT).exec(path)?.[0] ?? "";
};

/** `path` without the separators at its end, keeping a root such as `/` or `C:\` whole. */
export const trimSeparators = (path: string): string => {
	const root = rootOf(path);
	const ends = pathStyleOf(path) === "windows" ? /[\\/]+$/u : /\/+$/u;
	return root + path.slice(root.length).replace(ends, "");
};

/** The names in `path` after its root, such as ["Users", "me"]. */
export const namesOf = (path: string): string[] =>
	path
		.slice(rootOf(path).length)
		.split(pathStyleOf(path) === "windows" ? /[\\/]/u : "/")
		.filter(Boolean);

const fromHome = (text: string): boolean =>
	text === "~" || text.startsWith("~/") || text.startsWith("~\\");

/** Whether `text` reads as a path: a full one, or one from home. */
export const isPathText = (text: string): boolean => isAbsolutePath(text) || fromHome(text);

/**
 * `text` with a leading `~` read as `home`. Anything else, or any text
 * while the home folder isn't known, null or empty, comes back as it is.
 * After a Windows home, the rest takes its separators.
 */
export const expandHome = (text: string, home: string | null): string => {
	if (!home || !fromHome(text)) return text;
	if (text === "~") return home;
	const rest = text.slice(2);
	return joinPath(
		home,
		pathStyleOf(home) === "windows" ? rest.replace(/[\\/]/gu, separatorOf(home)) : rest
	);
};

/** `path` with the home folder shown as `~`, for a sentence. Windows reads names in any case. */
export const contractHome = (path: string, home: string | null): string => {
	if (!home) return path;
	const base = trimSeparators(home);
	if (base === rootOf(base)) return path;
	const windows = pathStyleOf(base) === "windows";
	const head = path.slice(0, base.length);
	if (windows ? head.toLowerCase() !== base.toLowerCase() : head !== base) return path;
	const next = path.charAt(base.length);
	if (next === "") return "~";
	return next === "/" || (windows && next === "\\") ? `~${path.slice(base.length)}` : path;
};

/** The folder `path` is in, or null for a root such as `/` or `C:\` and for a bare name. */
export const parentOf = (path: string): string | null => {
	const trimmed = trimSeparators(path);
	const root = rootOf(trimmed);
	const rest = trimmed.slice(root.length);
	if (!rest) return null;
	const cut = lastSeparator(rest, pathStyleOf(trimmed));
	return cut < 0 ? root || null : root + rest.slice(0, cut);
};

/** The last name in `path`, or "" for a root. */
export const baseName = (path: string): string => {
	const trimmed = trimSeparators(path);
	const rest = trimmed.slice(rootOf(trimmed).length);
	return rest.slice(lastSeparator(rest, pathStyleOf(trimmed)) + 1);
};

/** The last name in `path`, or the whole path for a root or "", for a label. */
export const nameOf = (path: string): string => baseName(path) || path;

/** `name` inside `folder`, with the folder's own separator. */
export const joinPath = (folder: string, name: string): string => {
	const ends = pathStyleOf(folder) === "windows" ? /[\\/]$/u : /\/$/u;
	return ends.test(folder) ? folder + name : folder + separatorOf(folder) + name;
};

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
	if (/^[A-Za-z]:$/u.test(text)) return { folder: `${text}\\`, shown: `${text}\\`, name: "" };
	const cut = lastSeparator(text, pathStyleOf(text));
	const shown = text.slice(0, cut + 1);
	const folder = shown.length <= rootOf(text).length ? shown : text.slice(0, cut);
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

/** What a suggestion puts in the field: its path, and a separator after a folder on Tab. */
export const pickText = (typed: TypedPath, entry: FolderEntry, completing: boolean): string =>
	typed.shown +
	entry.name +
	(completing && entry.folder ? (typed.shown.endsWith("\\") ? "\\" : "/") : "");

/** A checkout's tag, such as "git · main", or null for a folder that isn't one. */
export const checkoutTag = (entry: FolderEntry): string | null => {
	if (!entry.checkout) return null;
	const branch = entry.checkout.branch;
	return branch ? t("git · {{branch}}", { branch }) : "git";
};

/** Why a new folder can't take `name` in a folder of `style`, or null when it can. */
export const folderNameProblem = (name: string, style: PathStyle = "posix"): string | null => {
	const trimmed = name.trim();
	if (!trimmed) return t("Name the new folder.");
	if (trimmed.includes("/") || (style === "windows" && trimmed.includes("\\")))
		return t("A folder's name can't have a slash.");
	if (trimmed === "." || trimmed === "..") return t("A folder can't be called . or ..");
	return null;
};
