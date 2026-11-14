// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Files dropped or pasted on a field in an agent's pane that holds none,
// such as an answer field, go to the pane's composer as if dropped there.
// The pane makes one of these, its composer takes what comes, and the
// field sends. In a pane with no composer nothing takes them, so the
// field says they can't go in.

import { createContext, useContext } from "react";

import type { DraggedFile } from "@/agent-view/file-drag";

/** Files on their way to the composer, as they came. */
export type PaneFile =
	/** Dropped from the Finder. */
	| { readonly type: "paths"; readonly paths: readonly string[] }
	/** Dragged out of a timeline. */
	| { readonly type: "timeline"; readonly file: DraggedFile }
	/** Dropped as the web view reads them. */
	| { readonly type: "dropped"; readonly files: readonly File[] }
	/** Pasted, which the composer reads from the clipboard first. */
	| { readonly type: "pasted"; readonly files: readonly File[] };

/** A pane's way from its fields to its composer. */
export type PaneFiles = {
	/** Hands files to the composer. False when the pane has none. */
	readonly send: (file: PaneFile) => boolean;
	/** Takes every file sent from now on. Returns the undo. */
	readonly take: (taker: (file: PaneFile) => void) => () => void;
};

export const createPaneFiles = (): PaneFiles => {
	let taker: ((file: PaneFile) => void) | null = null;
	return {
		send: (file) => {
			if (taker === null) return false;
			taker(file);
			return true;
		},
		take: (next) => {
			taker = next;
			return () => {
				if (taker === next) taker = null;
			};
		},
	};
};

/** The pane's, or null outside an agent's pane. */
export const PaneFilesContext = createContext<PaneFiles | null>(null);

export const usePaneFiles = (): PaneFiles | null => useContext(PaneFilesContext);
