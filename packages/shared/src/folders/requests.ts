// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The folder requests, as calls of a data source: list one folder on a
// device, for a path field's suggestions and the folder browser, and make
// one, for the browser's New folder. A device's `maxd` answers both for
// the app and for phones, never for an agent's tools.

import type { DeviceId, FolderList } from "../bindings";
import type { Ask, Result } from "../data/request-source";
import type { ActionResult } from "../data/source";

/** What a listing shows besides folders. */
export type ListOptions = {
	/** Whether files show too, for a field that wants a file. */
	readonly files: boolean;
	/** Whether names that start with a dot show. */
	readonly hidden: boolean;
	/**
	 * Lists only the names that hold this, ignoring case, those that start
	 * with it first. The device keeps them before it cuts a big folder
	 * short, so a name far down the folder still comes. Empty or left out,
	 * every name lists.
	 */
	readonly filter?: string;
};

/** The folder calls of a data source. */
export type FolderCalls = {
	/**
	 * Lists the folder at `path` on `device`, which may start with `~`.
	 * Answers `too_old` from a `maxd` that can't list folders.
	 */
	readonly listFolder: (
		device: DeviceId,
		path: string,
		options: ListOptions
	) => Promise<Result<FolderList>>;
	/** Makes one folder at `path` on `device`, in a folder that's there. */
	readonly makeFolder: (device: DeviceId, path: string) => Promise<ActionResult>;
};

/** The folder calls, each one request over `ask`. */
export const folderCalls = (ask: Ask): FolderCalls => ({
	listFolder: (device, path, { files, hidden, filter }) =>
		ask({ kind: "list_folder", device, path, files, hidden, filter: filter || null }),
	makeFolder: (device, path) => ask({ kind: "make_folder", device, path }),
});
