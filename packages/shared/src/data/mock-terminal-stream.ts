// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The terminals' bytes in the sample data, so the browser build shows
// working terminals with no `maxd` behind it. A shell prints a prompt in
// the agent's folder and echoes what you type. A few commands run:
// `pnpm test` prints a failing run over a few seconds, `pnpm dev` prints a
// log line every second until Ctrl-C, and `sudo true` asks for a password
// the way a real prompt does, with the terminal in password mode until you
// send a line, which it never echoes or keeps. `gh secret set` asks for a
// secret the same way, `ssh devbox` asks yes or no about a new host, and
// `python3` reads lines at its prompt and takes Ctrl-C without ending, so
// Stop does nothing to it. `vim` and `git commit` take the whole screen
// until you quit with `:q` or `:wq`, and `pnpm create vite` asks you to
// pick a framework with the arrow keys, reading each key as you press it.
// A command that reads what you type and printed nothing for a second
// waits for you, as `maxd` says. A command you ran with `!` keeps what it
// printed, which its card shows once it ends. A shell on a device with
// Shell integration on marks its prompts and commands, OSC 133, as `maxd`
// passes them on. An agent's session is a shell that takes
// the commands you run with `!` as `maxd` writes them in, and tells how
// each ended with what it printed between its marks, as a block. A line
// you type at its prompt yourself becomes a block too, unless it starts
// with a space.

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
	/** Whether a shell it starts marks its prompts and commands. */
	readonly marks?: (terminal: Terminal) => boolean;
	/**
	 * A line you typed at a shell's prompt yourself. A session answers what
	 * to tell once it ends, so it's a block of its card, and any other shell null.
	 */
	readonly typed?: (terminal: TerminalId, line: string) => ((end: BlockEnd) => void) | null;
};

/** What a command you ran with `!` printed, as its card keeps it. */
export type Printed = {
	/** Its lines, with colors and styles and no other escapes. A last line break makes no line. */
	readonly text: string;
	readonly lines: number;
	/** Whether `text` holds only the last part of what it printed. */
	readonly truncated: boolean;
};

/** How a command the session ran ended, as its block keeps it. */
export type BlockEnd = {
	/** Its exit code, or null when the shell lost it. */
	readonly code: number | null;
	readonly printed: Printed;
	/** How many passwords went in at prompts that hid them. */
	readonly secrets: number;
	/** Whether your Stop or Force stop ended it. */
	readonly stopped?: boolean;
	/** The shell's folder once it ended. */
	readonly cwd: string;
};

/** The sample stream, and what the terminal calls tell it. */
export type MockTerminalStream = TerminalStream & {
	/** Starts what a terminal runs, once it opened, after `replay` when a shell had a past. */
	readonly start: (terminal: Terminal, replay?: string) => void;
	/**
	 * Writes a command into a session's shell at its prompt, as `maxd`
	 * does, and tells `ended` how it went. Answers the offset its output
	 * starts at, or null when the shell isn't there.
	 */
	readonly command: (
		terminal: TerminalId,
		line: string,
		ended: (end: BlockEnd) => void
	) => number | null;
	/** Whether a shell waits at its prompt with nothing typed, so it can take a command. */
	readonly idle: (terminal: TerminalId) => boolean;
	/** The folder a shell is in now. */
	readonly cwd: (terminal: TerminalId) => string | undefined;
	/**
	 * Sends Ctrl-C, the way your Stop does, or with `force` kills what runs
	 * in a session and leaves the shell, the way Force stop does.
	 */
	readonly interrupt: (terminal: TerminalId, force?: boolean) => void;
	/** Ends it and forgets what it printed. */
	readonly end: (terminal: TerminalId) => void;
	/** Whether you named it, so commands no longer change its title. */
	readonly named: (terminal: TerminalId, named: boolean) => void;
	/** What a command you ran with `!` printed so far. */
	readonly printed: (terminal: TerminalId) => Printed | undefined;
};

/** The signal Ctrl-C sends. */
const SIGINT = 2;

/** The signal Force stop sends. */
const SIGKILL = 9;

/** How long a command that reads what you type prints nothing before it waits for you. */
const QUIET = 1000;

/** How much each sample terminal keeps, like the device's buffer. */
const KEPT = 256 * 1024;

const CTRL_C = "\x03";
const CTRL_D = "\x04";
const CTRL_L = "\x0c";
const ESC = "\x1b";
const DIM = "\x1b[2m";
const RED = "\x1b[31m";
const GREEN = "\x1b[32m";
const CYAN = "\x1b[36m";
const BLUE = "\x1b[34m";
const BOLD = "\x1b[1m";
const OFF = "\x1b[0m";

/** Into and out of the alternate screen, as a full-screen program goes. */
const SCREEN_ON = "\x1b[?1049h";
const SCREEN_OFF = "\x1b[?1049l";

/** Application cursor keys on with the cursor hidden, as a picker asks, and back. */
const PICKER_ON = "\x1b[?1h\x1b[?25l";
const PICKER_OFF = "\x1b[?1l\x1b[?25h";

/** The arrow keys, as a terminal sends them in either cursor mode. */
const UP = new Set(["\x1b[A", "\x1bOA", "k"]);
const DOWN = new Set(["\x1b[B", "\x1bOB", "j"]);

/** A command's lines, each after a wait in milliseconds, and its exit code. */
type Script = {
	readonly lines: readonly (readonly [wait: number, text: string])[];
	readonly exit: number;
};

/** What a command does. */
type Plan =
	| ({ readonly type: "script" } & Script)
	| { readonly type: "forever"; readonly head: string; readonly line: (n: number) => string }
	| {
			readonly type: "password";
			readonly prompt: string;
			/** What it prints once the line went, after it. */
			readonly done?: string;
	  }
	| {
			/** Asks a question on a line of its own and ends on your answer, or at Ctrl-D. */
			readonly type: "ask";
			readonly head: string;
			readonly prompt: string;
			/** The answers the prompt names, as `maxd` reads them off it. */
			readonly choices: readonly string[];
			readonly answer: (line: string | null) => Script;
	  }
	| {
			/** Reads lines at its prompt until `exit()` or Ctrl-D, and takes Ctrl-C without ending. */
			readonly type: "repl";
			readonly head: string;
			readonly prompt: string;
			readonly interrupted: string;
	  }
	| {
			/**
			 * Takes the whole screen, as vim does, showing `lines` over a status
			 * line, and ignores Ctrl-C. `:wq` or `:x` quits saved, `:q` or `:q!`
			 * without, and `after` says what it prints then.
			 */
			readonly type: "screen";
			readonly lines: readonly string[];
			readonly status: string;
			readonly after: (saved: boolean) => Script;
	  }
	| {
			/** Asks you to pick one of `options` with the arrow keys and Enter, as create-vite does. */
			readonly type: "picker";
			readonly head: string;
			readonly question: string;
			readonly options: readonly string[];
			readonly picked: (option: string) => Script;
	  }
	| { readonly type: "cd"; readonly to: string }
	| { readonly type: "exit" }
	| { readonly type: "clear" };

const baseName = (path: string) => path.replace(/\/+$/, "").split("/").pop() || path;

const clock = (at: Timestamp) => new Date(at).toTimeString().slice(0, 8);

const header = (project: string, script: string, command: string) =>
	`\r\n${DIM}> ${project}@0.1.0 ${script}${OFF}\r\n${DIM}> ${command}${OFF}\r\n\r\n`;

const HMR = ["/src/App.tsx", "/src/routes/home.tsx", "/src/styles.css", "/src/api/cache.ts"];

/** Where `cd` goes from `cwd`: home, a path from the root or home, or one under `cwd`. */
const cdTarget = (cwd: string, to: string | undefined): string => {
	if (!to || to === "~") return "~";
	const start = to.startsWith("/") || to.startsWith("~") ? [] : cwd.split("/");
	const parts = [...start];
	for (const part of to.split("/")) {
		if (part === "" && parts.length > 0) continue;
		if (part === ".") continue;
		if (part === "..") {
			if (parts.length > 1) parts.pop();
			continue;
		}
		parts.push(part);
	}
	return parts.join("/") || "/";
};

/** A word with the quotes `maxd` puts around a folder taken off. */
const unquoted = (word: string) => word.replace(/^'(.*)'$/, "$1");

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
			return { type: "password", prompt: "Password:" };
		case "ssh devbox":
			return {
				type: "ask",
				head:
					"The authenticity of host 'devbox (192.0.2.10)' can't be established.\r\n" +
					"ED25519 key fingerprint is SHA256:Qm4bX0r1zZk2cP9vT7yWq5nL8sE3dA6fH1jK4uV0x8.\r\n" +
					"This key is not known by any other names.\r\n",
				prompt: "Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
				choices: ["yes", "no"],
				answer: (line) =>
					line?.trim() === "yes"
						? {
								exit: 0,
								lines: [
									[
										0,
										"Warning: Permanently added 'devbox' (ED25519) to the list of known hosts.\r\n",
									],
									[600, "Connection to devbox closed.\r\n"],
								],
							}
						: { exit: 255, lines: [[0, "Host key verification failed.\r\n"]] },
			};
		case "python3":
			return {
				type: "repl",
				head:
					"Python 3.13.1 (main, Dec  3 2024, 17:59:52) [Clang 16.0.0] on darwin\r\n" +
					'Type "help", "copyright", "credits" or "license" for more information.\r\n',
				prompt: ">>> ",
				interrupted: "\r\nKeyboardInterrupt\r\n",
			};
		case "pnpm install":
			return {
				type: "script",
				exit: 0,
				lines: [
					[0, `Lockfile is up to date, resolution step is skipped\r\n`],
					[900, `Already up to date\r\n`],
					[2400, `\r\n${DIM}Done in 4.2s${OFF}\r\n`],
				],
			};
		case "git commit":
			return {
				type: "screen",
				lines: [
					"",
					`${BLUE}# Please enter the commit message for your changes. Lines starting${OFF}`,
					`${BLUE}# with '#' will be ignored, and an empty message aborts the commit.${OFF}`,
					`${BLUE}#${OFF}`,
					`${BLUE}# On branch main${OFF}`,
					`${BLUE}# Changes to be committed:${OFF}`,
					`${BLUE}#\tmodified:   src/usage/popover.tsx${OFF}`,
					`${BLUE}#${OFF}`,
				],
				status: `".git/COMMIT_EDITMSG" 8L, 262B`,
				after: (saved) =>
					saved
						? {
								exit: 0,
								lines: [
									[
										0,
										"[main 3f2a1c9] Show only the windows the plan reports\r\n" +
											" 1 file changed, 4 insertions(+), 2 deletions(-)\r\n",
									],
								],
							}
						: {
								exit: 1,
								lines: [[0, "Aborting commit due to empty commit message.\r\n"]],
							},
			};
		case "pnpm create vite":
			return {
				type: "picker",
				head:
					`${DIM}│${OFF}\r\n` +
					`${GREEN}◇${OFF}  Project name:\r\n` +
					`${DIM}│${OFF}  ${DIM}vite-project${OFF}\r\n` +
					`${DIM}│${OFF}\r\n`,
				question: "Select a framework:",
				options: ["Vanilla", "Vue", "React", "Preact", "Lit", "Svelte", "Solid"],
				picked: (option) => ({
					exit: 0,
					lines: [
						[
							0,
							`${DIM}│${OFF}\r\n${GREEN}◇${OFF}  Scaffolding project in ./vite-project...\r\n`,
						],
						[
							700,
							`${DIM}│${OFF}\r\n${DIM}└${OFF}  Done. Now run:\r\n\r\n` +
								`  cd vite-project\r\n  pnpm install\r\n  pnpm dev\r\n` +
								`\r\n${DIM}A ${option} template.${OFF}\r\n`,
						],
					],
				}),
			};
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
	if (words[0] === "cd") {
		// `maxd` writes `cd -- <folder>` for the folder chip.
		const to = words[1] === "--" ? words.slice(2) : words.slice(1);
		return { type: "cd", to: cdTarget(cwd, to.length ? unquoted(to.join(" ")) : undefined) };
	}
	if (words[0] === "vim" && words[1])
		return {
			type: "screen",
			lines: [
				"# Notes",
				"",
				"- The popover reads its rows from the account.",
				"- A plan with one window shows one bar.",
			],
			status: `"${words[1]}" 4L, 96B`,
			after: () => ({ exit: 0, lines: [] }),
		};
	if (words[0] === "sudo") return { type: "password", prompt: "[sudo] password for me:" };
	if (words[0] === "gh" && words[1] === "secret" && words[2] === "set" && words[3])
		return {
			type: "password",
			prompt: `${GREEN}?${OFF} ${BOLD}Paste your secret:${OFF} `,
			done: `${GREEN}✓${OFF} Set Actions secret ${words[3]} for owner/repo\r\n`,
		};
	if (words[0] === "echo")
		return { type: "script", exit: 0, lines: [[0, `${words.slice(1).join(" ")}\r\n`]] };
	if (words[0] === "export") return { type: "script", exit: 0, lines: [] };
	return {
		type: "script",
		exit: 127,
		lines: [[0, `zsh: command not found: ${words[0]}\r\n`]],
	};
};

/** A command that runs, and how to stop it. */
type Job = {
	readonly command: string;
	readonly stop: () => void;
	/** What it does with Ctrl-C, when it takes it and goes on. */
	readonly keepsOn?: () => void;
};

/** A session's command while it runs: what it printed so far, and who hears how it ends. */
type Block = {
	text: string;
	secrets: number;
	readonly ended: (end: BlockEnd) => void;
};

type State = {
	readonly id: TerminalId;
	readonly shell: boolean;
	cwd: string;
	readonly chunks: { offset: number; bytes: Uint8Array }[];
	end: number;
	attached: boolean;
	readonly listeners: Set<(event: TerminalEvent) => void>;
	/** What you typed at the prompt since the last Enter. */
	line: string;
	job: Job | null;
	/** Set while a password prompt waits, with what to do once a line comes. */
	secret: (() => void) | null;
	/** Set while a command reads a line it echoes, with what to do with it, or null at Ctrl-D. */
	reply: ((line: string | null) => void) | null;
	/** Set while a command reads keys one at a time, with what to do with each. */
	keys: ((key: string) => void) | null;
	/** Counts down to the command waiting for you, while it reads and prints nothing. */
	quiet: ReturnType<typeof setTimeout> | null;
	named: boolean;
	/** What it printed as text, up to what it keeps. */
	text: string;
	/** Whether `text` lost its start to the limit. */
	dropped: boolean;
	/** Whether it marks its prompts and commands. */
	readonly marks: boolean;
	/** The session's command that runs, between its C and D marks. */
	block: Block | null;
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
	marks,
	typed: typedLine,
}: MockStreamHooks): MockTerminalStream => {
	const states = new Map<TerminalId, State>();

	const emit = (state: State, event: TerminalEvent) => {
		if (!state.attached) return;
		for (const listener of [...state.listeners]) listener(event);
	};

	/**
	 * Prints `text`. What a picker redraws, or a full screen shows, isn't
	 * `kept`: it stays out of what a card keeps of the command.
	 */
	const print = (state: State, text: string, kept = true) => {
		const bytes = utf8(text);
		if (bytes.length === 0) return;
		if (!state.shell && kept) {
			state.text += text;
			if (state.text.length > KEPT) {
				state.text = state.text.slice(-KEPT);
				state.dropped = true;
			}
		}
		if (state.block && kept) state.block.text += text;
		const chunk = { offset: state.end, bytes };
		state.chunks.push(chunk);
		state.end += bytes.length;
		while (state.chunks.length > 1 && state.end - state.chunks[0]!.offset > KEPT)
			state.chunks.shift();
		emit(state, { type: "output", ...chunk });
	};

	/** A mark, OSC 133, in a shell that prints them. */
	const mark = (state: State, body: string) => (state.marks ? `\x1b]133;${body}\x07` : "");

	const prompt = (state: State) =>
		print(
			state,
			`${mark(state, "A")}${CYAN}${BOLD}${baseName(state.cwd)}${OFF} ${DIM}%${OFF} ${mark(state, "B")}`
		);

	/**
	 * Sets what runs, or nothing at the prompt, so the terminal says it's
	 * busy, and a shell you didn't name takes the command as its title.
	 */
	const foreground = (state: State, job: Job | null) => {
		state.job = job;
		edit(state.id, (terminal) => ({
			...terminal,
			busy: job !== null,
			title: state.shell && !state.named ? (job?.command ?? "zsh") : terminal.title,
		}));
	};

	/** Tells a session's block how it ended, before the D mark, so the mark isn't in its text. */
	const endBlock = (state: State, code: number | null, stopped = false) => {
		const block = state.block;
		if (!block) return;
		state.block = null;
		block.ended({
			code,
			printed: shown(block.text),
			secrets: block.secrets,
			cwd: state.cwd,
			...(stopped ? { stopped } : {}),
		});
	};

	/** It no longer waits for you: it took a line, or ended. */
	const answered = (state: State) => {
		if (state.quiet) clearTimeout(state.quiet);
		state.quiet = null;
		if (find(state.id)?.waiting)
			edit(state.id, (terminal) => ({ ...terminal, waiting: false, choices: [] }));
	};

	/**
	 * The command reads what you type, so it waits for you once it printed
	 * nothing for a while, with the answers its prompt names.
	 */
	const reads = (state: State, choices: readonly string[] = []) => {
		answered(state);
		state.quiet = setTimeout(() => {
			state.quiet = null;
			edit(state.id, (terminal) => ({ ...terminal, waiting: true, choices: [...choices] }));
		}, QUIET);
	};

	/** The command no longer reads keys or holds the whole screen, and its modes go back. */
	const plain = (state: State) => {
		state.keys = null;
		const terminal = find(state.id);
		if (!terminal?.full_screen && !terminal?.reads_keys) return;
		if (terminal.full_screen) print(state, SCREEN_OFF, false);
		if (terminal.reads_keys) print(state, PICKER_OFF, false);
		edit(state.id, (now) => ({
			...now,
			full_screen: false,
			reads_keys: false,
			app_cursor: false,
		}));
	};

	/** The command ended with `code`, or a signal when null, by your Stop when `stopped`. */
	const finish = (
		state: State,
		code: number | null,
		signal: number | null = null,
		stopped = false
	) => {
		state.secret = null;
		state.reply = null;
		plain(state);
		answered(state);
		if (state.shell) {
			foreground(state, null);
			endBlock(state, code, stopped);
			print(state, mark(state, `D;${code ?? ""}`));
			prompt(state);
			return;
		}
		state.job = null;
		edit(state.id, (terminal) => ({
			...terminal,
			busy: false,
			password_prompt: false,
			status: { state: "exited", code, signal, ...(stopped ? { stopped } : {}) },
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
		/** Prints a script's lines over time, then ends with its code, at once when it waits for none. */
		const play = (script: Script): boolean => {
			let at = 0;
			for (const [wait, text] of script.lines) {
				at += wait;
				if (at === 0) print(state, text);
				else later(at, () => print(state, text));
			}
			if (at === 0) finish(state, script.exit);
			else later(at + 50, () => finish(state, script.exit));
			return at === 0;
		};
		let keepsOn: (() => void) | undefined;
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
			case "cd":
				state.cwd = plan.to;
				return finish(state, 0);
			case "password":
				print(state, plan.prompt);
				edit(state.id, (terminal) => ({ ...terminal, password_prompt: true }));
				reads(state);
				state.secret = () => {
					answered(state);
					edit(state.id, (terminal) => ({ ...terminal, password_prompt: false }));
					if (state.block) state.block.secrets += 1;
					print(state, ` 🔒 sent\r\n${plan.done ?? ""}`);
					finish(state, 0);
				};
				break;
			case "ask":
				print(state, plan.head + plan.prompt);
				reads(state, plan.choices);
				state.reply = (line) => {
					state.reply = null;
					answered(state);
					play(plan.answer(line));
				};
				break;
			case "repl": {
				const ready = () => {
					print(state, plan.prompt);
					reads(state);
				};
				print(state, plan.head);
				ready();
				state.reply = (line) => {
					answered(state);
					if (line === null || line.trim() === "exit()" || line.trim() === "quit()")
						return finish(state, 0);
					ready();
				};
				keepsOn = () => {
					state.line = "";
					print(state, plan.interrupted);
					ready();
				};
				break;
			}
			case "screen": {
				const rows = find(state.id)?.rows ?? 24;
				const bottom = (text: string) =>
					print(state, `\x1b[${rows};1H\x1b[2K${text}`, false);
				const filler = Array.from(
					{ length: Math.max(0, rows - 1 - plan.lines.length) },
					() => `${BLUE}~${OFF}`
				);
				print(
					state,
					`${SCREEN_ON}\x1b[H\x1b[2J${[...plan.lines, ...filler].join("\r\n")}`,
					false
				);
				bottom(plan.status);
				edit(state.id, (terminal) => ({ ...terminal, full_screen: true }));
				let typing: string | null = null;
				state.keys = (key) => {
					if (typing === null) {
						if (key === ":") {
							typing = "";
							bottom(":");
						}
						return;
					}
					if (key === ESC) {
						typing = null;
						return bottom("");
					}
					if (key === "\x7f" || key === "\b") {
						typing = typing === "" ? null : typing.slice(0, -1);
						return bottom(typing === null ? "" : `:${typing}`);
					}
					if (key !== "\r") {
						if (key.length === 1 && key >= " ") bottom(`:${(typing += key)}`);
						return;
					}
					const said = typing.trim();
					typing = null;
					const saved = ["w", "wq", "wq!", "x", "x!"].includes(said);
					if (!saved && !["q", "q!", "qa", "qa!"].includes(said))
						return bottom(`${RED}E492: Not an editor command: ${said}${OFF}`);
					if (said === "w") return bottom(plan.status.replace(/\d+B$/, "written"));
					plain(state);
					play(plan.after(saved));
				};
				keepsOn = () => bottom("Type  :qa  and press <Enter> to exit Vim");
				break;
			}
			case "picker": {
				const n = plan.options.length;
				let at = 0;
				const options = () =>
					plan.options
						.map((option, i) =>
							i === at
								? `\x1b[2K${DIM}│${OFF}  ${GREEN}●${OFF} ${option}\r\n`
								: `\x1b[2K${DIM}│  ○ ${option}${OFF}\r\n`
						)
						.join("") + `\x1b[2K${CYAN}└${OFF}\r\n`;
				print(state, plan.head);
				print(state, `${PICKER_ON}${CYAN}◆${OFF}  ${plan.question}\r\n${options()}`, false);
				edit(state.id, (terminal) => ({ ...terminal, reads_keys: true, app_cursor: true }));
				reads(state);
				state.keys = (key) => {
					if (UP.has(key) || DOWN.has(key)) {
						at = (at + (UP.has(key) ? n - 1 : 1)) % n;
						print(state, `\x1b[${n + 1}A\r${options()}`, false);
						return reads(state);
					}
					if (key !== "\r") return;
					answered(state);
					print(state, `\x1b[${n + 2}A\r\x1b[J`, false);
					print(
						state,
						`${GREEN}◇${OFF}  ${plan.question}\r\n${DIM}│${OFF}  ${DIM}${plan.options[at]}${OFF}\r\n`
					);
					plain(state);
					play(plan.picked(plan.options[at]!));
				};
				break;
			}
			case "forever": {
				print(state, plan.head);
				let n = 0;
				timers.push(setInterval(() => print(state, plan.line(n++)), 1000));
				break;
			}
			case "script":
				// A command that prints all at once is done at once.
				if (play(plan)) return;
				break;
		}
		foreground(state, { command, stop, keepsOn });
	};

	/**
	 * Ctrl-C you typed, or your Stop when `by` says so, which ends what runs
	 * unless it takes Ctrl-C and goes on. Force stop ends it either way.
	 */
	const interrupt = (state: State, by: "keys" | "stop" | "force" = "keys") => {
		if (state.job?.keepsOn && by !== "force") return state.job.keepsOn();
		plain(state);
		print(state, by === "force" ? "\r\n" : "^C\r\n");
		state.line = "";
		if (state.job) {
			state.job.stop();
			if (state.secret)
				edit(state.id, (terminal) => ({ ...terminal, password_prompt: false }));
			const signal = by === "force" ? SIGKILL : SIGINT;
			if (!state.shell) return finish(state, null, signal, by !== "keys");
			state.secret = null;
			state.reply = null;
			answered(state);
			foreground(state, null);
			endBlock(state, 128 + signal, by !== "keys");
			// A shell says what a signal stopped exited with 128 and the signal.
			print(state, mark(state, `D;${128 + signal}`));
		}
		prompt(state);
	};

	const enter = (state: State) => {
		const line = state.line;
		state.line = "";
		print(state, "\r\n");
		if (!line.trim()) return prompt(state);
		print(state, mark(state, "C"));
		// A session tells its card of a line you typed, unless it starts with a space.
		const ended =
			state.marks && !line.startsWith(" ") ? typedLine?.(state.id, line.trim()) : null;
		if (ended) state.block = { text: "", secrets: 0, ended };
		run(state, line.trim());
	};

	/**
	 * A key typed into a command that reads a line, which it echoes as a
	 * terminal in line mode does. The echo is output, so it no longer waits.
	 */
	const read = (state: State, char: string) => {
		const reply = state.reply;
		if (!reply) return;
		answered(state);
		if (char === "\r" || char === "\n") {
			const line = state.line;
			state.line = "";
			print(state, "\r\n");
			reply(line);
		} else if (char === CTRL_D && state.line === "") {
			print(state, "\r\n");
			reply(null);
		} else if (char === "\x7f" || char === "\b") {
			if (state.line) {
				state.line = state.line.slice(0, -1);
				print(state, "\b \b");
			}
		} else if (char >= " ") {
			state.line += char;
			print(state, char);
		}
	};

	// Escape sequences, such as the arrow keys and the markers around a
	// paste, are left out, but for a command that reads keys, which takes
	// each whole; the sample shell has no line editing.
	const typed = (state: State, text: string) => {
		if (find(state.id)?.status.state !== "running") return;
		for (let i = 0; i < text.length; i++) {
			const char = text[i]!;
			if (char === ESC) {
				const from = i;
				if (text[i + 1] === "[") {
					i += 2;
					while (i < text.length && !/[@-~]/.test(text[i]!)) i++;
				} else if (text[i + 1] === "O") i += 2;
				else if (i + 1 < text.length) i++;
				state.keys?.(text.slice(from, i + 1));
				continue;
			}
			if (char === CTRL_C) {
				interrupt(state);
				continue;
			}
			if (state.keys) {
				state.keys(char);
				continue;
			}
			if (state.secret) {
				// What you type at a password prompt is never echoed or kept.
				if (char === "\r" || char === "\n") state.secret();
				continue;
			}
			if (state.reply) {
				read(state, char);
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
		start: (terminal, replay) => {
			const shell = terminal.kind.type === "shell" || terminal.kind.type === "session";
			const state: State = {
				id: terminal.id,
				shell,
				cwd: terminal.cwd,
				chunks: [],
				end: 0,
				attached: false,
				listeners: new Set(),
				line: "",
				job: null,
				secret: null,
				reply: null,
				keys: null,
				quiet: null,
				named: false,
				text: "",
				dropped: false,
				marks: shell && (marks?.(terminal) ?? false),
				block: null,
			};
			states.set(terminal.id, state);
			if (terminal.kind.type === "run") run(state, terminal.kind.command);
			else {
				if (replay) print(state, replay);
				prompt(state);
			}
		},
		command: (id, line, ended) => {
			const state = stateOf(id);
			if (!state?.shell) return null;
			// The shell echoes the line `maxd` writes, and marks where its output starts.
			print(state, `${line}\r\n${mark(state, "C")}`);
			const start = state.end;
			state.block = { text: "", secrets: 0, ended };
			run(state, line.trim());
			return start;
		},
		idle: (id) => {
			const state = stateOf(id);
			return (
				state !== undefined &&
				state.shell &&
				!state.job &&
				!state.secret &&
				!state.reply &&
				state.line === ""
			);
		},
		cwd: (id) => stateOf(id)?.cwd,
		interrupt: (id, force = false) => {
			const state = stateOf(id);
			if (state) interrupt(state, force ? "force" : "stop");
		},
		end: (id) => {
			const state = stateOf(id);
			state?.job?.stop();
			if (state?.quiet) clearTimeout(state.quiet);
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
