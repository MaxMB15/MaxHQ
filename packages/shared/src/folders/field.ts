// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A path field as you type in it. It shows what you typed and hands the
// form the path with `~` read as the device's home folder, which it learns
// from the first listing that names one. It lists the folder you're typing
// in after a short pause, as the composer's `@` menu waits, and asks only
// for the names that hold the name you type, so one far down a big folder
// still comes. It drops an answer for a folder you've since left, and
// narrows a listing that came whole to more of the name without asking
// again.

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import type { DeviceId, FolderEntry, FolderListing } from "../bindings";
import { MENU_DELAY_MS } from "../composer/menu";
import { expandHome, fitting, type PathWants, splitTyped, type TypedPath } from "./path";
import type { FolderCalls } from "./requests";

/** A path field's text and suggestions. */
export type PathField = {
	/** What the field shows. */
	readonly text: string;
	/** Puts text in the field: what you typed, or a path you picked or dropped. */
	readonly change: (text: string) => void;
	/** The folder and name the suggestions are for, or null for text that isn't a path. */
	readonly typed: TypedPath | null;
	/** The entries that fit what you typed, best first. */
	readonly entries: readonly FolderEntry[];
	/** The device's home folder, once a listing told it. */
	readonly home: string | null;
	/** Asks again for what the field shows, as after a folder was made. */
	readonly reload: () => void;
};

/** One folder's listing on one device, or none when it didn't come. */
export type Found = {
	readonly device: DeviceId;
	readonly folder: string;
	/** Whether it lists files too. */
	readonly files: boolean;
	/** The name it was asked to hold, in lower case. */
	readonly filter: string;
	/** The reload it came after. */
	readonly round: number;
	readonly listing: FolderListing | null;
};

/**
 * Whether `found` answers for `name` as well: it was asked for that name,
 * or for part of it and came whole, so it holds every name that does.
 */
export const covers = (found: Found, name: string): boolean => {
	const lower = name.toLowerCase();
	if (found.filter === lower) return true;
	return found.listing !== null && !found.listing.truncated && lower.includes(found.filter);
};

/**
 * The field for `value`, a path on `device`. `lists` says whether that
 * device's `maxd` lists folders. Without it, or with no device, the field
 * suggests nothing and reads no `~`.
 */
export const usePathField = ({
	calls,
	device,
	lists,
	value,
	onChange,
	wants,
}: {
	calls: Pick<FolderCalls, "listFolder">;
	device: DeviceId | null;
	lists: boolean;
	value: string;
	onChange: (value: string) => void;
	wants: PathWants;
}): PathField => {
	const [text, setText] = useState(value);
	const [seen, setSeen] = useState(value);
	const [found, setFound] = useState<Found | null>(null);
	const [round, setRound] = useState(0);
	const [home, setHome] = useState<{ device: DeviceId; home: string } | null>(null);
	const asked = useRef(0);
	const latest = useRef({ value, onChange });
	useLayoutEffect(() => {
		latest.current = { value, onChange };
	});

	const known = home !== null && home.device === device ? home.home : null;
	// A value the form set itself, such as a reset, shows as it is.
	if (value !== seen) {
		setSeen(value);
		if (expandHome(text, known) !== value) setText(value);
	}

	// The form gets what you typed with `~` read, again once home is known.
	// It gets it before the next key lands, which would otherwise read the
	// form's older value as one the form set itself.
	useLayoutEffect(() => {
		const next = expandHome(text, known);
		if (next !== latest.current.value) latest.current.onChange(next);
	}, [text, known]);

	const typed = lists && device !== null ? splitTyped(text) : null;
	const folder = typed?.folder ?? null;
	const files = wants === "file";
	// The listing found is for this folder, and still answers for the name.
	const here =
		found !== null &&
		found.device === device &&
		found.folder === folder &&
		found.files === files;
	const answers = here && found.round === round && covers(found, typed?.name ?? "");
	// The name to ask for, or null while what was found answers.
	const filter = typed !== null && !answers ? typed.name.toLowerCase() : null;
	useEffect(() => {
		if (device === null || folder === null || filter === null) return;
		const ask = ++asked.current;
		const done = (listing: FolderListing | null) =>
			setFound({ device, folder, files, filter, round, listing });
		const timer = setTimeout(() => {
			calls.listFolder(device, folder, { files, hidden: true, filter }).then(
				(result) => {
					if (asked.current !== ask) return;
					const listing =
						result.status === "ok" && result.data.status === "listed"
							? result.data.listing
							: null;
					done(listing);
					// A device that can't tell its home leaves ~ as typed.
					if (listing?.home) setHome({ device, home: listing.home });
				},
				() => {
					if (asked.current === ask) done(null);
				}
			);
		}, MENU_DELAY_MS);
		return () => clearTimeout(timer);
	}, [calls, device, files, folder, filter, round]);

	// Until a new answer comes, the last one for this folder narrows to
	// what you type.
	const listing = here ? found.listing : null;
	return {
		text,
		change: setText,
		typed,
		entries: typed && listing?.exists ? fitting(listing.entries, typed.name, wants) : [],
		home: known,
		reload: useCallback(() => setRound((n) => n + 1), []),
	};
};
