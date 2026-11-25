// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// MaxHQ's own folder browser, for a device whose folders the system picker
// can't see. It lists one folder at a time through that device's `maxd`,
// starting at what the field holds, or at the nearest folder above it that
// is there. A breadcrumb and an up button move about, a click picks an
// entry and a double click opens a folder. Choose takes the picked entry,
// or the folder shown when it wants a folder and nothing is picked. Each
// app draws it its own way: a dialog on the Mac, a full screen on a phone.

import { useCallback, useEffect, useRef, useState } from "react";

import type { DeviceId, FolderEntry, FolderListing } from "../bindings";
import { t } from "../i18n";
import { errorText } from "../timeline/store";
import {
	baseName,
	folderNameProblem,
	isPathText,
	joinPath,
	namesOf,
	parentOf,
	pathStyleOf,
	rootOf,
	type PathWants,
} from "./path";
import type { FolderCalls, ListOptions } from "./requests";

/** One step of the breadcrumb: a name, and the folder it opens. */
export type Crumb = { readonly name: string; readonly path: string };

/** The breadcrumb for a full path, from its root, such as `/` or `C:`, down to its last name. */
export const crumbsOf = (path: string): Crumb[] => {
	const root = rootOf(path) || "/";
	const crumbs: Crumb[] = [
		{ name: root === "/" ? root : root.replace(/[\\/]$/u, ""), path: root },
	];
	for (const name of namesOf(path))
		crumbs.push({ name, path: joinPath(crumbs[crumbs.length - 1]!.path, name) });
	return crumbs;
};

/** Where the browser opens: what the field holds when it's a path, else home. */
export const startOf = (value: string): string => {
	const text = value.trim();
	return isPathText(text) ? text : "~";
};

/** A listing the browser reached, with the name to pick in it. */
export type Reached =
	| { readonly ok: true; readonly listing: FolderListing; readonly selected: string | null }
	| { readonly ok: false; readonly reason: string };

/** What a `maxd` from before folders lists say. */
export const tooOld = (): string => t("This device's maxd can't list folders yet.");

/** The most folders the browser walks up from a start that isn't there. */
const MAX_CLIMB = 64;

/**
 * Lists `path` on `device`, or the nearest folder above it that's there.
 * Walking up from a missing folder or from a file picks the name it came
 * from, so a file field opens on its file.
 */
export const reachFolder = async (
	calls: Pick<FolderCalls, "listFolder">,
	device: DeviceId,
	path: string,
	options: ListOptions
): Promise<Reached> => {
	let asked = path;
	let selected: string | null = null;
	for (let climbed = 0; climbed <= MAX_CLIMB; climbed++) {
		const result = await calls.listFolder(device, asked, options);
		if (result.status === "error") return { ok: false, reason: errorText(result.error) };
		if (result.data.status === "too_old") return { ok: false, reason: tooOld() };
		const listing = result.data.listing;
		const parent = parentOf(listing.path);
		if (listing.exists || parent === null) return { ok: true, listing, selected };
		if (climbed === 0) selected = baseName(listing.path);
		asked = parent;
	}
	return { ok: false, reason: t("Couldn't find a folder to open.") };
};

/** Whether `entry` is what a field that `wants` takes. */
const takes = (entry: FolderEntry, wants: PathWants): boolean =>
	entry.folder === (wants === "folder");

/**
 * What Choose would take: the picked entry when the field takes it, else
 * the folder shown for a field that wants a folder. Null while there's
 * nothing to take.
 */
export const chosenPath = (
	listing: FolderListing | null,
	selected: string | null,
	wants: PathWants
): string | null => {
	if (listing === null || !listing.exists) return null;
	const entry = listing.entries.find((e) => e.name === selected);
	if (entry && takes(entry, wants)) return joinPath(listing.path, entry.name);
	return wants === "folder" ? listing.path : null;
};

/** The browser as it stands. */
export type FolderBrowserState = {
	/** The folder shown, or null before the first listing comes. */
	readonly listing: FolderListing | null;
	/** True while a listing is on its way. */
	readonly loading: boolean;
	/** Why the last listing didn't come, or null. */
	readonly failed: string | null;
	/** Whether names that start with a dot show. */
	readonly hidden: boolean;
	/** The entry picked in the folder shown, by name. */
	readonly selected: string | null;
	/** What Choose takes, or null while it takes nothing. */
	readonly chosen: string | null;
};

/** The browser and what it can do. */
export type FolderBrowser = FolderBrowserState & {
	/** Opens a folder, by its full path. */
	readonly go: (path: string) => void;
	/** Opens the folder above the one shown. */
	readonly up: () => void;
	/** Picks an entry by name, or none. */
	readonly select: (name: string | null) => void;
	/** Opens a folder entry, or picks a file one. */
	readonly open: (entry: FolderEntry) => void;
	/** Shows or hides names that start with a dot. */
	readonly showHidden: (hidden: boolean) => void;
	/**
	 * Makes a folder called `name` in the one shown and picks it. Answers
	 * why it couldn't, or null once it's there.
	 */
	readonly make: (name: string) => Promise<string | null>;
};

/**
 * The browser for `device`, opening at `start`. A listing that comes back
 * after a newer one was asked for is dropped.
 */
export const useFolderBrowser = ({
	calls,
	device,
	start,
	wants,
}: {
	calls: FolderCalls;
	device: DeviceId;
	start: string;
	wants: PathWants;
}): FolderBrowser => {
	const [listing, setListing] = useState<FolderListing | null>(null);
	const [loading, setLoading] = useState(true);
	const [failed, setFailed] = useState<string | null>(null);
	const [hidden, setHidden] = useState(false);
	const [selected, setSelected] = useState<string | null>(null);
	const asked = useRef(0);

	// Asks for a listing. Loading starts true, so the first ask needn't set it.
	const request = useCallback(
		(path: string, show: boolean, pick: string | null) => {
			const ask = ++asked.current;
			void reachFolder(calls, device, path, { files: wants === "file", hidden: show }).then(
				(reached) => {
					if (asked.current !== ask) return;
					setLoading(false);
					if (!reached.ok) return setFailed(reached.reason);
					setFailed(null);
					setListing(reached.listing);
					setSelected(pick ?? reached.selected);
				},
				(error: unknown) => {
					if (asked.current !== ask) return;
					setLoading(false);
					setFailed(error instanceof Error ? error.message : String(error));
				}
			);
		},
		[calls, device, wants]
	);

	useEffect(() => request(start, false, null), [request, start]);

	const load = (path: string, show: boolean, pick: string | null = null) => {
		setLoading(true);
		request(path, show, pick);
	};

	const shown = listing?.path ?? null;
	return {
		listing,
		loading,
		failed,
		hidden,
		selected,
		chosen: chosenPath(listing, selected, wants),
		go: (path) => load(path, hidden),
		up: () => {
			const parent = shown === null ? null : parentOf(shown);
			if (parent !== null && shown !== null) load(parent, hidden, baseName(shown));
		},
		select: setSelected,
		open: (entry) => {
			if (!entry.folder) return setSelected(entry.name);
			if (shown !== null) load(joinPath(shown, entry.name), hidden);
		},
		showHidden: (show) => {
			setHidden(show);
			if (shown !== null) load(shown, show, selected);
		},
		make: async (name) => {
			const problem = folderNameProblem(name, shown === null ? "posix" : pathStyleOf(shown));
			if (problem !== null) return problem;
			if (shown === null) return t("Wait for the folder to load.");
			const folder = joinPath(shown, name.trim());
			const result = await calls.makeFolder(device, folder);
			if (result.status === "error") return errorText(result.error);
			if (result.data.status === "refused") return result.data.reason;
			load(shown, hidden, name.trim());
			return null;
		},
	};
};
