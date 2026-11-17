// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The terminals' bytes in the sample data, so the browser build shows
// working terminals with no `maxd` behind it. A shell prints a prompt in
// the agent's folder and echoes what you type. A few commands run:
// `pnpm test` prints a failing run over a few seconds, `pnpm dev` prints a
// log line every second until Ctrl-C, and `sudo true` asks for a password
// the way a real prompt does, with the terminal in password mode until you
// send a line, which it never echoes or keeps. A command you ran with `!`
// keeps what it printed, which its card shows once it ends.

import type { Terminal, TerminalId, Timestamp } from "../bindings";
import { utf8 } from "../notes/merge";
import type { TerminalEvent, TerminalStream } from "../terminal/stream";

/** What the stream needs from the sample terminals. */
export type MockStreamHooks = {
	readonly find: (id: TerminalId) => Terminal | undefined;
	readonly edit: (id: TerminalId, next: (terminal: Terminal) => Terminal) => void;
	readonly now: () => Timestamp;
	/** A command you ran with `!` ended, with its status already set. */
	readonly exited?: (terminal: TerminalId) => void;
};

/** What a command you ran with `!` printed, as its card keeps it. */
export type Printed = {
	/** Its lines, with colors and styles and no other escapes. A last line break makes no line. */
	readonly text: string;
	readonly lines: number;
	/** Whether `text` holds only the last part of what it printed. */
	readonly truncated: boolean;
};

/** The sample stream, and what the terminal calls tell it. */
export type MockTerminalStream = TerminalStream & {
	/** Starts what a terminal runs, once it opened. */
	readonly start: (terminal: Terminal) => void;
	/** Sends Ctrl-C, the way an interrupt does. */
	readonly interrupt: (terminal: TerminalId) => void;
	/** Whether a command runs in it, which a close without force refuses. */
	readonly busy: (terminal: TerminalId) => boolean;
	/** Ends it and forgets what it printed. */
	readonly end: (terminal: TerminalId) => void;
	/** Whether you named it, so commands no longer change its title. */
	readonly named: (terminal: TerminalId, named: boolean) => void;
	/** What a command you ran with `!` printed so far. */
	readonly printed: (terminal: TerminalId) => Printed | undefined;
};

/** The signal Ctrl-C sends. */
const SIGINT = 2;

/** How much each sample terminal keeps, like the device's buffer. */
const KEPT = 256 * 1024;

const CTRL_C = "\x03";
const CTRL_L = "\x0c";
const ESC = "\x1b";
const DIM = "\x1b[2m";
const RED = "\x1b[31m";
const GREEN = "\x1b[32m";
const CYAN = "\x1b[36m";
const BOLD = "\x1b[1m";
const OFF = "\x1b[0m";

/** A command's lines, each after a wait in milliseconds, and its exit code. */
type Script = {
	readonly lines: readonly (readonly [wait: number, text: string])[];
	readonly exit: number;
};

/** What a command does. */
type Plan =
	| ({ readonly type: "script" } & Script)
	| { readonly type: "forever"; readonly head: string; readonly line: (n: number) => string }
	| { readonly type: "password" }
	| { readonly type: "exit" }
	| { readonly type: "clear" };

const baseName = (path: string) => path.replace(/\/+$/, "").split("/").pop() || path;

const clock = (at: Timestamp) => new Date(at).toTimeString().slice(0, 8);

const header = (project: string, script: string, command: string) =>
	`\r\n${DIM}> ${project}@0.1.0 ${script}${OFF}\r\n${DIM}> ${command}${OFF}\r\n\r\n`;

const HMR = ["/src/App.tsx", "/src/routes/home.tsx", "/src/styles.css", "/src/api/cache.ts"];

const planFor = (command: string, cwd: string, now: () => Timestamp): Plan => {
	const project = baseName(cwd);
	const words = command.trim().split(/\s+/);
	switch (words.join(" ")) {
		case "pnpm test":
			return {
				type: "script",
				exit: 1,
				lines: [
					[0, header(project, "test", "vitest run")],
					[300, ` ${CYAN}${BOLD}RUN${OFF}  v3.2.4 ${DIM}${cwd}${OFF}\r\n\r\n`],
					[500, ` ${GREEN}✓${OFF} src/format.test.ts ${DIM}(12 tests) 18ms${OFF}\r\n`],
					[400, ` ${GREEN}✓${OFF} src/parse.test.ts ${DIM}(31 tests) 42ms${OFF}\r\n`],
					[
						600,
						` ${RED}✗${OFF} src/cache.test.ts ${DIM}(2 tests | 1 failed) 9ms${OFF}\r\n`,
					],
					[100, `   ${RED}✗${OFF} expires entries after the TTL\r\n\r\n`],
					[
						300,
						`${RED}${BOLD} FAIL ${OFF} src/cache.test.ts > expires entries after the TTL\r\n` +
							`${RED}AssertionError: expected 'stale' to be undefined${OFF}\r\n` +
							` ${CYAN}❯${OFF} src/cache.test.ts:48:22\r\n\r\n`,
					],
					[
						400,
						` Test Files  ${RED}${BOLD}1 failed${OFF} | ${GREEN}${BOLD}2 passed${OFF} (3)\r\n` +
							`      Tests  ${RED}${BOLD}1 failed${OFF} | ${GREEN}${BOLD}44 passed${OFF} (45)\r\n` +
							`   Duration  2.84s\r\n` +
							`${RED} ELIFECYCLE ${OFF} Test failed. See above for more details.\r\n`,
					],
				],
			};
		case "pnpm dev":
			return {
				type: "forever",
				head:
					header(project, "dev", "vite") +
					`  ${GREEN}${BOLD}VITE${OFF} v7.1.3  ${DIM}ready in 412 ms${OFF}\r\n\r\n` +
					`  ${GREEN}➜${OFF}  ${BOLD}Local${OFF}:   ${CYAN}http://localhost:5173/${OFF}\r\n` +
					`  ${GREEN}➜${OFF}  ${DIM}Network: use --host to expose${OFF}\r\n\r\n`,
				line: (n) =>
					`${DIM}${clock(now())}${OFF} ${CYAN}${BOLD}[vite]${OFF} ${GREEN}hmr update${OFF} ${DIM}${HMR[n % HMR.length]}${OFF}\r\n`,
			};
		case "sudo true":
			return { type: "password" };
		case "exit":
			return { type: "exit" };
		case "clear":
			return { type: "clear" };
		case "ls":
			return {
				type: "script",
				exit: 0,
				lines: [
					[
						0,
						`README.md     package.json  ${CYAN}${BOLD}src${OFF}           ${CYAN}${BOLD}tests${OFF}\r\n`,
					],
				],
			};
		case "pwd":
			return { type: "script", exit: 0, lines: [[0, `${cwd}\r\n`]] };
	}
	if (words[0] === "echo")
		return { type: "script", exit: 0, lines: [[0, `${words.slice(1).join(" ")}\r\n`]] };
	return {
		type: "script",
		exit: 127,
		lines: [[0, `zsh: command not found: ${words[0]}\r\n`]],
	};
};

/** A command that runs, and how to stop it. */
type Job = { readonly command: string; readonly stop: () => void };

type State = {
	readonly id: TerminalId;
	readonly shell: boolean;
	readonly cwd: string;
	readonly chunks: { offset: number; bytes: Uint8Array }[];
	end: number;
	attached: boolean;
	readonly listeners: Set<(event: TerminalEvent) => void>;
	/** What you typed at the prompt since the last Enter. */
	line: string;
	job: Job | null;
	/** Set while a password prompt waits, with what to do once a line comes. */
	secret: (() => void) | null;
	named: boolean;
	/** What it printed as text, up to what it keeps. */
	text: string;
	/** Whether `text` lost its start to the limit. */
	dropped: boolean;
};

/** Escape sequences other than colors and styles, which a card leaves out. */
// eslint-disable-next-line no-control-regex
const NOT_SGR = /\x1b\[[0-?]*[ -/]*[@-ln-~]|\x1b[^[]/g;

/** What a card shows of a command's text: SGR escapes kept, one `\n` per line, no last break. */
const shown = (text: string): Printed => {
	const body = text
		.replace(NOT_SGR, "")
		.replace(/\r\n/g, "\n")
		.replace(/\r/g, "")
		.replace(/\n$/, "");
	return { text: body, lines: body === "" ? 0 : body.split("\n").length, truncated: false };
};

/**
 * What you typed, from its UTF-8 bytes. This package has no `TextDecoder`,
 * and a key comes whole, so each message reads on its own.
 */
const typedText = (bytes: Uint8Array): string => {
	try {
		return decodeURIComponent(
			Array.from(bytes, (byte) => `%${byte.toString(16).padStart(2, "0")}`).join("")
		);
	} catch {
		return String.fromCharCode(...bytes);
	}
};

/** The sample terminals' bytes. */
export const mockTerminalStream = ({
	find,
	edit,
	now,
	exited,
}: MockStreamHooks): MockTerminalStream => {
	const states = new Map<TerminalId, State>();

	const emit = (state: State, event: TerminalEvent) => {
		if (!state.attached) return;
		for (const listener of [...state.listeners]) listener(event);
	};

	const print = (state: State, text: string) => {
		const bytes = utf8(text);
		if (bytes.length === 0) return;
		if (!state.shell) {
			state.text += text;
			if (state.text.length > KEPT) {
				state.text = state.text.slice(-KEPT);
				state.dropped = true;
			}
		}
		const chunk = { offset: state.end, bytes };
		state.chunks.push(chunk);
		state.end += bytes.length;
		while (state.chunks.length > 1 && state.end - state.chunks[0]!.offset > KEPT)
			state.chunks.shift();
		emit(state, { type: "output", ...chunk });
	};

	const prompt = (state: State) =>
		print(state, `${CYAN}${BOLD}${baseName(state.cwd)}${OFF} ${DIM}%${OFF} `);

	const retitle = (state: State, title: string) => {
		if (!state.named) edit(state.id, (terminal) => ({ ...terminal, title }));
	};

	/** The command ended with `code`, or a signal when null. */
	const finish = (state: State, code: number | null, signal: number | null = null) => {
		state.job = null;
		state.secret = null;
		if (state.shell) {
			retitle(state, "zsh");
			prompt(state);
			return;
		}
		edit(state.id, (terminal) => ({
			...terminal,
			password_prompt: false,
			status: { state: "exited", code, signal },
		}));
		exited?.(state.id);
	};

	const run = (state: State, command: string) => {
		const plan = planFor(command, state.cwd, now);
		const timers: ReturnType<typeof setTimeout>[] = [];
		const later = (wait: number, act: () => void) => timers.push(setTimeout(act, wait));
		const stop = () => {
			for (const timer of timers) clearTimeout(timer);
		};
		switch (plan.type) {
			case "clear":
				print(state, "\x1b[H\x1b[2J");
				return finish(state, 0);
			case "exit":
				if (!state.shell) return finish(state, 0);
				return edit(state.id, (terminal) => ({
					...terminal,
					status: { state: "exited", code: 0, signal: null },
				}));
			case "password":
				print(state, "Password:");
				edit(state.id, (terminal) => ({ ...terminal, password_prompt: true }));
				state.secret = () => {
					edit(state.id, (terminal) => ({ ...terminal, password_prompt: false }));
					print(state, " 🔒 sent\r\n");
					finish(state, 0);
				};
				break;
			case "forever": {
				print(state, plan.head);
				let n = 0;
				timers.push(setInterval(() => print(state, plan.line(n++)), 1000));
				break;
			}
			case "script": {
				let at = 0;
				for (const [wait, text] of plan.lines) {
					at += wait;
					if (at === 0) print(state, text);
					else later(at, () => print(state, text));
				}
				// A command that prints all at once is done at once.
				if (at === 0) return finish(state, plan.exit);
				later(at + 50, () => finish(state, plan.exit));
				break;
			}
		}
		state.job = { command, stop };
		if (state.shell) retitle(state, command);
	};

	const interrupt = (state: State) => {
		print(state, "^C\r\n");
		state.line = "";
		if (state.job) {
			state.job.stop();
			if (state.secret)
				edit(state.id, (terminal) => ({ ...terminal, password_prompt: false }));
			if (!state.shell) return finish(state, null, SIGINT);
			state.job = null;
			state.secret = null;
			retitle(state, "zsh");
		}
		prompt(state);
	};

	const enter = (state: State) => {
		const line = state.line;
		state.line = "";
		print(state, "\r\n");
		if (line.trim()) run(state, line.trim());
		else prompt(state);
	};

	// Escape sequences, such as the arrow keys and the markers around a
	// paste, are left out; the sample shell has no line editing.
	const typed = (state: State, text: string) => {
		if (find(state.id)?.status.state !== "running") return;
		for (let i = 0; i < text.length; i++) {
			const char = text[i]!;
			if (char === ESC) {
				if (text[i + 1] === "[") {
					i += 2;
					while (i < text.length && !/[@-~]/.test(text[i]!)) i++;
				} else i++;
				continue;
			}
			if (char === CTRL_C) {
				interrupt(state);
				continue;
			}
			if (state.secret) {
				// What you type at a password prompt is never echoed or kept.
				if (char === "\r" || char === "\n") state.secret();
				continue;
			}
			if (state.job || !state.shell) continue;
			if (char === "\r" || char === "\n") enter(state);
			else if (char === "\x7f" || char === "\b") {
				if (state.line) {
					state.line = state.line.slice(0, -1);
					print(state, "\b \b");
				}
			} else if (char === CTRL_L) {
				print(state, "\x1b[H\x1b[2J");
				prompt(state);
				print(state, state.line);
			} else if (char >= " ") {
				state.line += char;
				print(state, char);
			}
		}
	};

	const stateOf = (id: TerminalId) => states.get(id);

	return {
		start: (terminal) => {
			const state: State = {
				id: terminal.id,
				shell: terminal.kind.type === "shell",
				cwd: terminal.cwd,
				chunks: [],
				end: 0,
				attached: false,
				listeners: new Set(),
				line: "",
				job: null,
				secret: null,
				named: false,
				text: "",
				dropped: false,
			};
			states.set(terminal.id, state);
			if (terminal.kind.type === "shell") prompt(state);
			else run(state, terminal.kind.command);
		},
		interrupt: (id) => {
			const state = stateOf(id);
			if (state) interrupt(state);
		},
		busy: (id) => stateOf(id)?.job != null,
		end: (id) => {
			stateOf(id)?.job?.stop();
			states.delete(id);
		},
		named: (id, named) => {
			const state = stateOf(id);
			if (state) state.named = named;
		},
		printed: (id) => {
			const state = stateOf(id);
			if (!state || state.shell) return undefined;
			return { ...shown(state.text), truncated: state.dropped };
		},
		attach: async (id, after) => {
			const state = stateOf(id);
			if (!state || !find(id)) throw new Error("This terminal is gone.");
			state.attached = true;
			const first = state.chunks[0]?.offset ?? state.end;
			const from = after === null ? first : Math.max(after + 1, first);
			if (after !== null && after + 1 < first) emit(state, { type: "reset", from: first });
			for (const chunk of state.chunks) {
				const end = chunk.offset + chunk.bytes.length;
				if (end <= from) continue;
				emit(state, {
					type: "output",
					offset: Math.max(chunk.offset, from),
					bytes: chunk.bytes.subarray(Math.max(0, from - chunk.offset)),
				});
			}
		},
		detach: (id) => {
			const state = stateOf(id);
			if (state) state.attached = false;
		},
		resize: (id, { cols, rows }) => {
			if (find(id)) edit(id, (terminal) => ({ ...terminal, cols, rows }));
		},
		input: (id, bytes) => {
			const state = stateOf(id);
			if (state) typed(state, typedText(bytes));
		},
		listen: (id, listener) => {
			const state = stateOf(id);
			state?.listeners.add(listener);
			return () => state?.listeners.delete(listener);
		},
	};
};
