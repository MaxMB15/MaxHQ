// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: which ignored files a
// copy ticks before you change anything, the env files.
// The rest is MaxHQ's: any file can be ticked, a default tick also has to
// be 1 MiB or less, and the total is checked against a size worth a warning
// and the limits of the one transfer that carries the files.

import type { IgnoredFile } from "../data/source";

import { baseName } from "../folders/path";
/** The largest file a copy ticks by default. */
export const DEFAULT_COPY_MAX_BYTES = 1024 * 1024;

/** Past this total a copy warns that it'll take a while. */
export const COPY_WARN_BYTES = 100_000_000;

/** The most one transfer carries, as crates/transfer's `MAX_BYTES`. */
export const COPY_MAX_BYTES = 2 * 1024 * 1024 * 1024;

/**
 * The most files and folders one transfer carries, as crates/transfer's
 * `MAX_ENTRIES`. The folder that holds the files counts too.
 */
export const COPY_MAX_FILES = 20_000;

const DEFAULT_COPIED_FILE_NAME = /^(?:\.env(?:\..+)?|\.envrc|\.dev\.vars)$/;

/** Env files, which people almost always want on a new device. */
export const isDefaultCopied = (file: IgnoredFile): boolean => {
	const name = baseName(file.path) || file.path;
	return DEFAULT_COPIED_FILE_NAME.test(name) && file.size <= DEFAULT_COPY_MAX_BYTES;
};

/** The paths a copy ticks before you change anything. */
export const defaultTicks = (files: readonly IgnoredFile[]): Set<string> =>
	new Set(files.filter(isDefaultCopied).map((file) => file.path));

/** How much the ticked files come to, and whether a copy takes them. */
export type CopySize = {
	readonly files: number;
	readonly bytes: number;
	/** Fine, worth a warning, or more than one transfer carries. */
	readonly look: "fine" | "large" | "too_large";
};

/** The folders the paths sit in, each once, the way a transfer lists them. */
const foldersOf = (paths: readonly string[]): number => {
	const folders = new Set<string>();
	for (const path of paths) {
		for (let at = path.indexOf("/"); at !== -1; at = path.indexOf("/", at + 1)) {
			folders.add(path.slice(0, at));
		}
	}
	return folders.size;
};

export const copySize = (files: readonly IgnoredFile[], ticked: ReadonlySet<string>): CopySize => {
	const chosen = files.filter((file) => ticked.has(file.path));
	const bytes = chosen.reduce((sum, file) => sum + file.size, 0);
	const entries = 1 + chosen.length + foldersOf(chosen.map((file) => file.path));
	const look =
		bytes > COPY_MAX_BYTES || entries > COPY_MAX_FILES
			? "too_large"
			: bytes > COPY_WARN_BYTES
				? "large"
				: "fine";
	return { files: chosen.length, bytes, look };
};
