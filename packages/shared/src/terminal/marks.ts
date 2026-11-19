// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The marks a drawer shell prints around its prompts and commands, OSC
// 133, once `maxd` checked them and took its nonce out: `A` where the
// prompt starts, `B` where what you type starts, `C` where the command
// starts and `D;<exit>` where it ended. The drawer's terminal reads them
// to tell commands apart, the way `maxd` does. A shell that prints none
// has no commands here, and its terminal works as it did before.

/** One mark. */
export type ShellMark =
	| { readonly kind: "prompt" }
	| { readonly kind: "input" }
	| { readonly kind: "command" }
	| { readonly kind: "done"; readonly exit: number | null };

/** The mark an OSC 133 body such as `D;2` holds, or null for one the drawer doesn't use. */
export const readMark = (body: string): ShellMark | null => {
	const [kind, exit] = body.split(";");
	switch (kind) {
		case "A":
			return { kind: "prompt" };
		case "B":
			return { kind: "input" };
		case "C":
			return { kind: "command" };
		case "D":
			return { kind: "done", exit: exit && /^-?\d+$/.test(exit) ? Number(exit) : null };
		default:
			return null;
	}
};

/** What the marks say of one command. */
export type CommandMark = {
	/** What you typed, as the shell echoed it. */
	readonly command: string;
	/** How it exited, or null while it runs or when the shell didn't say. */
	readonly exit: number | null;
	readonly running: boolean;
};

/** Where one command's marks sit, as `P` places them in the terminal. */
export type CommandSpots<P> = {
	prompt: P | null;
	input: P | null;
	start: P | null;
	end: P | null;
	exit: number | null;
};

/** What a mark changed in a terminal's commands. */
export type MarkChange<C> = {
	/** Whether a command holds the mark's place now. */
	readonly kept: boolean;
	/** A prompt nothing ran from, which the list let go. */
	readonly dropped: C | null;
	/** A command that ended, said so or not. */
	readonly ended: C | null;
	/** A command that started. */
	readonly started: C | null;
};

const NOTHING = { kept: false, dropped: null, ended: null, started: null } as const;

const runs = <P>(command: CommandSpots<P> | undefined): boolean =>
	command !== undefined && command.start !== null && command.end === null;

/**
 * Takes in a mark at `at`, into `list`, a terminal's commands oldest first,
 * with `fresh` making a command with no marks. A prompt or a command that
 * starts ends one that ran without saying, as when a shell started inside
 * it, and a new prompt lets go of one nothing ran from.
 */
export const takeMark = <P, C extends CommandSpots<P>>(
	list: C[],
	mark: ShellMark,
	at: P,
	fresh: () => C
): MarkChange<C> => {
	const last = list.at(-1);
	const running = runs(last) ? last! : null;
	const waiting = last && last.start === null ? last : null;
	switch (mark.kind) {
		case "prompt": {
			if (running) running.end = at;
			if (waiting) list.pop();
			const next = fresh();
			next.prompt = at;
			list.push(next);
			return { kept: true, dropped: waiting, ended: running, started: null };
		}
		case "input":
			if (!waiting) return NOTHING;
			waiting.input = at;
			return { ...NOTHING, kept: true };
		case "command": {
			if (running) running.end = at;
			const next = waiting ?? fresh();
			if (!waiting) list.push(next);
			next.start = at;
			return { kept: true, dropped: null, ended: running, started: next };
		}
		case "done":
			if (!running) return NOTHING;
			running.end = at;
			running.exit = mark.exit;
			return { kept: true, dropped: null, ended: running, started: null };
	}
};
