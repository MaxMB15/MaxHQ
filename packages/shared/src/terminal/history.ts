// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// ↑ and ↓ in shell mode: the commands you ran with `!` in this agent's
// timeline, newest first and each once. No shell's own history file is
// read.

import type { TimelineState } from "../timeline/apply";

/** The commands you ran in this timeline, newest first, without repeats. */
export const runHistory = (state: Pick<TimelineState, "items" | "order">): string[] => {
	const seen = new Set<string>();
	const commands: string[] = [];
	for (let index = state.order.length - 1; index >= 0; index -= 1) {
		const item = state.items.get(state.order[index]!);
		if (item?.kind !== "user_run") continue;
		const command = item.command.trim();
		if (command === "" || seen.has(command)) continue;
		seen.add(command);
		commands.push(command);
	}
	return commands;
};

/** Where a walk through earlier commands stands. */
export type RunWalk = {
	/** The earlier command that shows, from 0 for the newest. */
	readonly index: number;
	/** What you had typed before the walk began, which ↓ past the newest brings back. */
	readonly typed: string;
};

/** What one step of a walk shows, and where the walk stands after it. */
export type RunStep = { readonly text: string; readonly walk: RunWalk | null };

/**
 * One ↑ or ↓ through earlier commands. ↑ goes back one, ↓ comes forward
 * one, and ↓ past the newest gives back what you had typed. Null when
 * there's nowhere to go, so the key does what it does anyway.
 */
export const walkRuns = (
	entries: readonly string[],
	walk: RunWalk | null,
	direction: "backward" | "forward",
	typed: string
): RunStep | null => {
	if (direction === "backward") {
		const index = walk === null ? 0 : walk.index + 1;
		const text = entries[index];
		if (text === undefined) return null;
		return { text, walk: { index, typed: walk?.typed ?? typed } };
	}
	if (walk === null) return null;
	if (walk.index === 0) return { text: walk.typed, walk: null };
	const index = walk.index - 1;
	return { text: entries[index] ?? walk.typed, walk: { ...walk, index } };
};
