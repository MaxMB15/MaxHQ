// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a path field does with something dropped on it from this Mac. On a
// field for this Mac, a folder goes in as it is, a file where a folder
// goes brings the folder it's in, and a file where a file goes brings
// itself. On a field for another device the path means nothing there, so
// the field says so, and when the dropped folder is a checkout of a
// project that device also has, offers that device's checkout. Nothing is
// ever copied: Copy project does that.

import type { DeviceId, ProjectId } from "../bindings";
import { t } from "../i18n";
import { sameRepoIn } from "../agent-view/move-agent";
import { all, type ViewState } from "../view/replica";
import { dispatcherHomes } from "../view/select";
import { baseName, contractHome, parentOf, type PathWants } from "./path";
import type { FolderCalls } from "./requests";

/** One path dropped from this Mac, and whether it's a folder. */
export type Dropped = { readonly path: string; readonly folder: boolean };

/** What a drop does. */
export type DropOutcome =
	/** The path goes in the field, with what to say about it. */
	| { readonly kind: "use"; readonly path: string; readonly note: string | null }
	/** The field takes no such thing, and says why. */
	| { readonly kind: "refuse"; readonly why: string }
	/** The drop is on this Mac and the field is for another device. */
	| { readonly kind: "elsewhere"; readonly folder: string; readonly why: string };

/**
 * What dropping `dropped` on a field that `wants` a folder or a file on
 * another device or this Mac (`here`) does. `home` is this Mac's home
 * folder, when known, for the sentence.
 */
export const dropOutcome = (
	dropped: Dropped,
	wants: PathWants,
	here: boolean,
	home: string | null
): DropOutcome => {
	const folder = dropped.folder ? dropped.path : (parentOf(dropped.path) ?? dropped.path);
	if (!here)
		return {
			kind: "elsewhere",
			folder,
			why: dropped.folder ? t("That folder is on this Mac.") : t("That file is on this Mac."),
		};
	if (wants === "file") {
		return dropped.folder
			? { kind: "refuse", why: t("Drop a file here, not a folder.") }
			: { kind: "use", path: dropped.path, note: null };
	}
	if (dropped.folder) return { kind: "use", path: dropped.path, note: null };
	return {
		kind: "use",
		path: folder,
		note: t("Uses the folder it's in, {{folder}}", { folder: contractHome(folder, home) }),
	};
};

/** What the field says while a drag is over it. */
export const dropHint = (wants: PathWants, here: boolean): string => {
	if (!here) return t("That folder is on this Mac.");
	return wants === "folder" ? t("Drop to use this folder") : t("Drop to use this file");
};

/**
 * The project the folder at `path` on this Mac checks out, which its
 * `maxd` tells from a listing of the folder it's in. Null for a folder
 * that isn't a checkout's root, or when the listing doesn't come.
 */
export const checkoutProject = async (
	calls: Pick<FolderCalls, "listFolder">,
	mac: DeviceId,
	path: string
): Promise<ProjectId | null> => {
	const parent = parentOf(path);
	if (parent === null) return null;
	const name = baseName(path);
	// Asked by name, so a folder past where a big listing ends still comes.
	const result = await calls
		.listFolder(mac, parent, { files: false, hidden: true, filter: name })
		.catch(() => null);
	if (result?.status !== "ok" || result.data.status !== "listed") return null;
	return result.data.listing.entries.find((e) => e.name === name)?.checkout?.project ?? null;
};

/**
 * The folder on `device` that checks out the same repository as
 * `project`, as the Move dialog matches them, or null when it has none.
 */
export const sameRepoFolder = (
	view: ViewState,
	project: ProjectId,
	device: DeviceId
): string | null => {
	const homes = dispatcherHomes(view);
	const folders = all(view, "workspace").filter(
		(w) => w.device === device && w.location.type === "folder" && !homes.has(w.id)
	);
	const match = sameRepoIn(view, project, folders);
	return match?.location.type === "folder" ? match.location.path : null;
};
