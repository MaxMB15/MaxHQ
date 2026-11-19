// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A path field as you type in it. It shows what you typed and hands the
// form the path with `~` read as the device's home folder, which it learns
// from the first listing. It lists the folder you're typing in after a
// short pause, as the composer's `@` menu waits, drops an answer for a
// folder you've since left, and narrows the listing to the name you type
// without asking again.

import { useEffect, useLayoutEffect, useRef, useState } from "react";

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
};

/** One folder's listing on one device, or none when it didn't come. */
type Found = {
	readonly device: DeviceId;
	readonly folder: string;
	readonly listing: FolderListing | null;
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
	useEffect(() => {
		if (device === null || folder === null) return;
		const ask = ++asked.current;
		const timer = setTimeout(() => {
			calls.listFolder(device, folder, { files, hidden: true }).then(
				(result) => {
					if (asked.current !== ask) return;
					const listing =
						result.status === "ok" && result.data.status === "listed"
							? result.data.listing
							: null;
					setFound({ device, folder, listing });
					if (listing) setHome({ device, home: listing.home });
				},
				() => {
					if (asked.current === ask) setFound({ device, folder, listing: null });
				}
			);
		}, MENU_DELAY_MS);
		return () => clearTimeout(timer);
	}, [calls, device, files, folder]);

	const listing =
		found !== null && found.device === device && found.folder === folder ? found.listing : null;
	return {
		text,
		change: setText,
		typed,
		entries: typed && listing?.exists ? fitting(listing.entries, typed.name, wants) : [],
		home: known,
	};
};
