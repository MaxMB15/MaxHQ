// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The copies the photo and file pickers make of what you pick, in folders of
// their own inside the app's caches folder. The box deletes a copy once it
// isn't needed, as staging.ts says when. Only a file under one of those
// folders counts as the app's own copy, told by its path, so a file the app
// didn't copy is never deleted.

/** The folders in the caches folder where the photo and file pickers put their copies. */
export const PICKER_FOLDERS = ["ImagePicker", "DocumentPicker"] as const;

/** Whether `path` is a copy a picker made inside the caches folder at `cache`. */
export const ownCopy = (path: string, cache: string): boolean => {
	const root = cache.replace(/\/+$/, "");
	if (!root.startsWith("/")) return false;
	for (const folder of PICKER_FOLDERS) {
		const inside = `${root}/${folder}/`;
		if (!path.startsWith(inside)) continue;
		// No step out of the folder, and a name at the end.
		return path
			.slice(inside.length)
			.split("/")
			.every((part) => part !== "" && part !== "." && part !== "..");
	}
	return false;
};

/**
 * A way to delete the file at `path` once, through `remove`, when it's a
 * picker's copy in the caches folder at `cache`, and nothing otherwise.
 */
export const discarder = (
	path: string | null,
	cache: string,
	remove: (path: string) => void
): (() => void) | undefined => {
	if (path === null || !ownCopy(path, cache)) return undefined;
	let gone = false;
	return () => {
		if (gone) return;
		gone = true;
		remove(path);
	};
};
