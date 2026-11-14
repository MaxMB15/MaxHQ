// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a hover card says, kept apart from how it looks so the wording can be
// tested for every state.

/** One label and value row in a hover card. */
export type Fact = {
	/** The label on the left, such as "Context". */
	readonly label: string;
	/** The value on the right. */
	readonly value: string;
};

/** How far one connection check got. */
export type CheckState = "passed" | "failed" | "running" | "not_run" | "unknown";

/** One check in the chain from this Mac to an agent. */
export type Check = {
	/** Stable across renders. */
	readonly id: CheckId;
	/** The one-word name under the hover card's bar, such as "SSH". */
	readonly short: string;
	/** The row title in the device view, such as "devbox on the tailnet". */
	readonly title: string;
	/** How far it got. */
	readonly state: CheckState;
	/** What it found, in a sentence. */
	readonly detail: string;
};

/** The checks, in the order they run. */
export type CheckId = "net" | "tailnet" | "peer" | "ping" | "ssh" | "maxd" | "agents";

/** The text of one hover card. */
export type HoverCard = {
	/** The bold first line. */
	readonly title: string;
	/** A sentence or two under the title. */
	readonly body?: string;
	/** Label and value rows. */
	readonly facts: readonly Fact[];
	/** The connection checks, drawn as a bar while an attempt runs. */
	readonly checks?: readonly Check[];
	/** The small last line, such as where the state came from. */
	readonly foot?: string;
};
