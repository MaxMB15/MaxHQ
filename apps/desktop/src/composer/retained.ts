// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Follows the behaviour of MIT-licensed code listed in THIRD_PARTY_NOTICES: a
// removed pill keeps its file, so undo brings both back. The file isn't held in
// a map until the draft goes away. It stays in the composer's attachments, and
// goes once the undo history no longer holds its pill.

import type { Node as ProseNode } from "@tiptap/pm/model";
import type { EditorState } from "@tiptap/pm/state";

import { fileKeyOfNode } from "./doc";

type Step = { readonly slice?: { readonly content?: ProseNode["content"] } };
type Item = { readonly step?: Step };
type Branch = {
	readonly items?: { forEach: (each: (item: Item) => void) => void };
};
type History = { readonly done?: Branch; readonly undone?: Branch };

const HISTORY_KEY = "history$";

const keysIn = (branch: Branch | undefined, into: Set<string>): boolean => {
	const items = branch?.items;
	if (typeof items?.forEach !== "function") return false;
	items.forEach((item) =>
		item.step?.slice?.content?.descendants((node) => {
			const key = fileKeyOfNode(node);
			if (key !== null) into.add(key);
		})
	);
	return true;
};

/**
 * The files whose pills undo or redo could bring back: each one in a step
 * the history keeps. Null when the history can't be read, so nothing goes
 * until the message is sent.
 */
export const heldFileKeys = (state: EditorState): ReadonlySet<string> | null => {
	const plugin = state.plugins.find((each) => (each as { key?: string }).key === HISTORY_KEY);
	if (!plugin) return new Set();
	const history = plugin.getState(state) as History | undefined;
	const held = new Set<string>();
	if (!keysIn(history?.done, held) || !keysIn(history?.undone, held)) return null;
	return held;
};

/**
 * The files to let go of: each one no pill in the draft names and the
 * history doesn't hold.
 */
export const releasedFiles = (input: {
	readonly keys: readonly string[];
	readonly placed: ReadonlySet<string>;
	readonly held: ReadonlySet<string> | null;
}): string[] => {
	const { held, placed } = input;
	if (held === null) return [];
	return input.keys.filter((key) => !placed.has(key) && !held.has(key));
};
