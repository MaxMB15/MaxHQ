// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Saving a file out of the app: through the share sheet on iOS, where Save to
// Files is, and into a folder the person picks on Android, whose share sheet
// takes no files. The Android folder comes through the system's document
// picker, so the file is a content:// document, never a path.

import { Directory, File, Paths } from "expo-file-system";
import { Share } from "react-native";

import { platform } from ".";

/** What to save: text, or a file the app holds, which moves rather than copies when `owned`. */
export type Saving = { readonly text: string } | { readonly file: File; readonly owned?: boolean };

/**
 * Saves `what` as a file named `name`. Resolves to whether the caller
 * should say it saved, which the share sheet does on its own and a picker
 * closed without a folder doesn't need.
 */
export const saveAs = async (name: string, mime: string, what: Saving): Promise<boolean> => {
	if (platform.savesByShare) {
		// Its own folder, so the copy takes the file's name without meeting the app's own files.
		const folder = new Directory(Paths.cache, "shared");
		folder.create({ intermediates: true, idempotent: true });
		const copy = new File(folder, name);
		if ("text" in what) await copy.write(what.text);
		else if (what.owned) await what.file.move(copy, { overwrite: true });
		else await what.file.copy(copy, { overwrite: true });
		await Share.share({ url: copy.uri });
		return false;
	}
	const folder = await Directory.pickDirectoryAsync().catch(() => null);
	// Closed without picking one.
	if (!folder) return false;
	const saved = folder.createFile(name, mime);
	if ("text" in what) await saved.write(what.text);
	else {
		await saved.write(await what.file.bytes());
		if (what.owned) what.file.delete();
	}
	return true;
};
