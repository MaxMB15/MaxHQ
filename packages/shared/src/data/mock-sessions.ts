// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The session calls of the sample data and of tests. Each agent has one
// shell its `!` commands share, opened by the first one. A command it
// runs adds a block to the agent's open card, which gets how it ended
// and what it printed, as `maxd` does. A command the session can't take
// opens a shell of its own, whose "You ran" card says why. A line you type
// at the session's prompt yourself adds a private block too, with what
// reads like a secret masked. A message you send closes the open cards,
// and says whether it took one's pill along.

import type {
	ActionOutcome,
	AgentId,
	DeviceId,
	FreshReason,
	ItemId,
	SessionBlock,
	Terminal,
	TerminalId,
	TerminalKind,
	Timestamp,
	UserInput,
} from "../bindings";
import type { SessionCalls } from "../terminal/session";
import type { TerminalSize } from "../terminal/requests";
import type { BlockEnd, MockTerminalStream } from "./mock-terminal-stream";
import type { MockTimelines } from "./mock-timeline";
import { MAC } from "./mock-view";
import type { ActionResult } from "./source";

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

const notFound = (what: string): ActionResult => ({
	status: "error",
	error: { kind: "not_found", what },
});

const answer = (data: ActionOutcome): ActionResult => ({ status: "ok", data });

/** What a `maxd` before sessions says, as one on protocol 47 does. */
export const NO_SESSIONS = "Sessions aren't ready yet.";

/** The session calls with no sessions behind them. */
export const noSessions: SessionCalls = {
	sessionRun: async () => refused(NO_SESSIONS),
	sessionShare: async () => refused(NO_SESSIONS),
	sessionCd: async () => refused(NO_SESSIONS),
	sessionEnd: async () => refused(NO_SESSIONS),
};

/** What a device says to moving a sealed card's block. */
export const CARD_SEALED = "That message went, so its commands can't change.";

/** The size a session opens at when the folder chip opens it. */
const CD_SIZE: TerminalSize = { cols: 100, rows: 24 };

// The sample ------------------------------------------------------------------

const MAXHQ_FOLDER = "/Users/me/Code/Github/Multi/MaxAIManager";
const DESKTOP_FOLDER = `${MAXHQ_FOLDER}/apps/desktop`;

const DIM = "\x1b[2m";
const RED = "\x1b[31m";
const GREEN = "\x1b[32m";
const CYAN = "\x1b[36m";
const BOLD = "\x1b[1m";
const OFF = "\x1b[0m";

const INSTALL_OUTPUT = [
	"Lockfile is up to date, resolution step is skipped",
	"Packages: +212",
	"Progress: resolved 212, reused 212, downloaded 0, added 212, done",
	"",
	`${DIM}Done in 4.2s${OFF}`,
].join("\n");

const SUDO_OUTPUT = "[sudo] password for me: 🔒 sent";

const TEST_OUTPUT = [
	`> maxhq-desktop@0.2.0 test ${DESKTOP_FOLDER}`,
	"> vitest run src/header",
	"",
	` RUN  v4.1.2 ${DESKTOP_FOLDER}`,
	"",
	` ${GREEN}✓${OFF} src/header/usage-rows.test.ts (6 tests) 12ms`,
	` ${RED}❯${OFF} src/header/usage-popover.test.tsx (9 tests | 1 failed) 151ms`,
	`   ${RED}×${OFF} hides the weekly row without a weekly window 38ms`,
	`     ${RED}→ expected "Weekly" not to be in the document${OFF}`,
	` ${GREEN}✓${OFF} src/header/account-chip.test.tsx (4 tests) 61ms`,
	"",
	` Test Files  ${RED}1 failed${OFF} | ${GREEN}2 passed${OFF} (3)`,
	`      Tests  ${RED}1 failed${OFF} | ${GREEN}18 passed${OFF} (19)`,
	"   Start at  14:52:31",
	"   Duration  9.06s",
].join("\n");

const YELLOW = "\x1b[33m";

const LOG_OUTPUT = [
	`${YELLOW}8c41d2e${OFF} Read the usage windows from the account`,
	`${YELLOW}5b0e7a3${OFF} Show the reset time under each bar`,
	`${YELLOW}a19f6c0${OFF} Add the usage meter to the header`,
].join("\n");

type SampleBlock = Omit<SessionBlock, "lines" | "output_truncated" | "start">;

const SAMPLE_BLOCKS: readonly SampleBlock[] = [
	{
		id: 1,
		command: "cd apps/desktop",
		cwd: MAXHQ_FOLDER,
		cwd_after: DESKTOP_FOLDER,
		private: false,
		process: { state: "exited", code: 0, signal: null },
		started_at: 0,
		duration_ms: 10,
		output: "",
		secret_prompts: 0,
	},
	{
		id: 2,
		command: "pnpm install",
		cwd: DESKTOP_FOLDER,
		cwd_after: null,
		private: false,
		process: { state: "exited", code: 0, signal: null },
		started_at: 6_000,
		duration_ms: 4_200,
		output: INSTALL_OUTPUT,
		secret_prompts: 0,
	},
	{
		id: 3,
		command: "sudo dscacheutil -flushcache",
		cwd: DESKTOP_FOLDER,
		cwd_after: null,
		private: true,
		process: { state: "exited", code: 0, signal: null },
		started_at: 21_000,
		duration_ms: 800,
		output: SUDO_OUTPUT,
		secret_prompts: 1,
	},
	{
		id: 4,
		command: "pnpm test",
		cwd: DESKTOP_FOLDER,
		cwd_after: null,
		private: false,
		process: { state: "exited", code: 1, signal: null },
		started_at: 34_000,
		duration_ms: 9_100,
		output: TEST_OUTPUT,
		secret_prompts: 0,
	},
	// Two lines you typed in the session's tab yourself, which start Private.
	{
		id: 5,
		command: "git log --oneline -3",
		cwd: DESKTOP_FOLDER,
		cwd_after: null,
		private: true,
		typed: true,
		process: { state: "exited", code: 0, signal: null },
		started_at: 52_000,
		duration_ms: 60,
		output: LOG_OUTPUT,
		secret_prompts: 0,
	},
	{
		id: 6,
		command: "export GH_TOKEN=•••",
		cwd: DESKTOP_FOLDER,
		cwd_after: null,
		private: true,
		typed: true,
		process: { state: "exited", code: 0, signal: null },
		started_at: 61_000,
		duration_ms: 10,
		output: "",
		secret_prompts: 0,
	},
];

/** How long the sample card ran, from its first block's start to its last one's end. */
export const SAMPLE_TOOK = 61_010;

const lineCount = (text: string) => (text === "" ? 0 : text.split("\n").length);

/** What the sample session printed before the app opened, so its tab shows its past. */
const sampleReplay = (): string => {
	let text = "";
	for (const block of SAMPLE_BLOCKS) {
		const folder = block.cwd.split("/").pop();
		text += `${CYAN}${BOLD}${folder}${OFF} ${DIM}%${OFF} ${block.command}\r\n`;
		if (block.output) text += `${block.output.replace(/\n/g, "\r\n")}\r\n`;
	}
	return text;
};

/**
 * The usage popover agent's session on the Mac: its terminal, the card it
 * holds open with four commands (a `cd`, an install, a private `sudo` and
 * a test that fails) and two lines you typed in its tab (a `git log` and
 * an `export` whose token is masked), and what its shell printed.
 */
export const SAMPLE_SESSION = {
	agent: "agent-usage-popover" as AgentId,
	device: MAC,
	terminal: "0199a7c2-0000-7000-8000-5e55100000a1" as TerminalId,
	item: "agent-usage-popover:session" as ItemId,
	/** The folder it started in. */
	started: MAXHQ_FOLDER,
	/** The folder its shell is in now. */
	cwd: DESKTOP_FOLDER,
	replay: sampleReplay(),
	/** The next block's ID. */
	next: SAMPLE_BLOCKS.length + 1,
} as const;

/** The sample session's terminal, which started `ago` before `now`. */
export const sampleSessionTerminal = (now: Timestamp, ago = 12 * 60_000): Terminal => ({
	id: SAMPLE_SESSION.terminal,
	agent: SAMPLE_SESSION.agent,
	device: SAMPLE_SESSION.device,
	kind: { type: "session" },
	moved: false,
	title: "zsh",
	cwd: SAMPLE_SESSION.started,
	status: { state: "running" },
	busy: false,
	marks: true,
	password_prompt: false,
	answers: true,
	started_at: now - ago,
	cols: 100,
	rows: 20,
});

/**
 * The sample session's open card. Each block's `started_at` counts from
 * the card's start, which the sample timeline adds.
 */
export const sampleSessionCard = () => ({
	kind: "user_session" as const,
	terminal: SAMPLE_SESSION.terminal,
	device: SAMPLE_SESSION.device,
	blocks: SAMPLE_BLOCKS.map((block): SessionBlock => ({
		...block,
		lines: lineCount(block.output),
		output_truncated: false,
		start: null,
	})),
	sealed: false,
	sent: false,
});

// The calls ---------------------------------------------------------------------

/** What the session calls need from the terminal calls around them. */
export type MockSessionHooks = {
	readonly now: () => Timestamp;
	readonly find: (id: TerminalId) => Terminal | undefined;
	/** The device and folder an agent runs in. */
	readonly placeOf: (agent: AgentId) => { device: DeviceId; cwd: string } | undefined;
	/**
	 * Opens a terminal for the agent, in `cwd` when given rather than the
	 * agent's folder, and a `!` run's card with why it ran on its own.
	 */
	readonly open: (
		agent: AgentId,
		kind: TerminalKind,
		size: TerminalSize,
		options: { readonly cwd?: string; readonly fresh?: FreshReason }
	) => ActionResult;
	/** Closes a terminal for good. */
	readonly close: (terminal: TerminalId) => void;
	readonly stream: Pick<MockTerminalStream, "command" | "idle" | "cwd">;
	/** Whether Shell integration is on for the device. */
	readonly marks: (device: DeviceId) => boolean;
	/** What a device says to an end without force while a command runs. */
	readonly stillRunning: string;
	readonly runs?: Pick<MockTimelines, "post" | "amend">;
};

/** The session calls, and what the other calls tell them. */
export type MockSessionCalls = SessionCalls & {
	/** A message went to the agent: its open cards close, and say whether it took them along. */
	readonly sealSessions: (agent: AgentId, input: UserInput) => void;
	/** A session's terminal closed, so a command still running in it ends with no code. */
	readonly sessionClosed: (terminal: TerminalId) => void;
	/**
	 * A line you typed at a session's prompt yourself: a private block of
	 * its open card, and what tells it how the line ended. Null for a
	 * terminal that isn't a session.
	 */
	readonly typedIn: (terminal: TerminalId, line: string) => ((end: BlockEnd) => void) | null;
};

/** A session while its shell runs. */
type Session = {
	readonly terminal: TerminalId;
	/** The card its commands go in, until a message closes it. */
	card: ItemId | null;
	/** The next block's ID, counted across the session's cards. */
	next: number;
	/** The block that runs, if one does. */
	running: { readonly item: ItemId; readonly block: number } | null;
};

/** Whether a command can't go to the session as typed, and why. */
const reasonIn = (command: string): FreshReason | null => {
	// A line break or a tab would reach the shell as keys.
	// eslint-disable-next-line no-control-regex
	if (/[\x00-\x1f\x7f]/.test(command)) return "lines";
	if (/^\s*exec(\s|$)/.test(command)) return "exec";
	return null;
};

/** What `maxd` shows for a secret it masks in a line you typed. */
const MASK = "•••";

/**
 * A line you typed, with a value set for a name that reads like a secret
 * masked, as `maxd` masks it: `export GH_TOKEN=•••`, `--password •••`.
 */
export const maskedLine = (line: string): string =>
	line
		.replace(
			/\b([A-Za-z_][A-Za-z0-9_]*(?:TOKEN|SECRET|PASSWORD|PASSWD|KEY)[A-Za-z0-9_]*)=("[^"]*"|'[^']*'|\S+)/gi,
			`$1=${MASK}`
		)
		.replace(
			/(--(?:password|passwd|token|secret)(?:=|\s+))(?!-)("[^"]*"|'[^']*'|\S+)/gi,
			`$1${MASK}`
		);

/** A folder as one word for `cd`. */
const quoted = (path: string) => `'${path.replace(/'/g, `'\\''`)}'`;

/** The session calls over the sample terminals and timelines. */
export const mockSessionCalls = (hooks: MockSessionHooks): MockSessionCalls => {
	const { now, find, stream, runs } = hooks;
	const sessions = new Map<AgentId, Session>();
	/** Each agent's cards no message closed yet. */
	const openCards = new Map<AgentId, Set<ItemId>>();
	let cards = 0;
	let seeded = false;

	/** Picks up the sample session the first time it's needed. */
	const seed = () => {
		if (seeded) return;
		seeded = true;
		if (find(SAMPLE_SESSION.terminal)?.status.state !== "running") return;
		sessions.set(SAMPLE_SESSION.agent, {
			terminal: SAMPLE_SESSION.terminal,
			card: SAMPLE_SESSION.item,
			next: SAMPLE_SESSION.next,
			running: null,
		});
		openCards.set(SAMPLE_SESSION.agent, new Set([SAMPLE_SESSION.item]));
	};

	const opened = (result: ActionResult): TerminalId | null =>
		result.status === "ok" && result.data.status === "terminal_opened"
			? result.data.terminal
			: null;

	/** Sets a block's fields in its card, and the card's status from all its blocks. */
	const amendBlock = (
		agent: AgentId,
		item: ItemId,
		block: number,
		change: (block: SessionBlock) => SessionBlock
	) =>
		runs?.amend(agent, item, (card) => {
			if (card.kind !== "user_session") return card;
			const blocks = card.blocks.map((each) => (each.id === block ? change(each) : each));
			const running = blocks.some((each) => each.process.state === "running");
			return {
				...card,
				blocks,
				status: running ? "running" : "completed",
				completed_at: running ? null : now(),
			};
		});

	/** Forgets a session whose shell is gone. A command still running in it ends with no code. */
	const forget = (agent: AgentId, session: Session) => {
		sessions.delete(agent);
		const running = session.running;
		if (!running) return;
		amendBlock(agent, running.item, running.block, (block) => ({
			...block,
			process: { state: "exited", code: null, signal: null },
			duration_ms: Math.max(0, now() - block.started_at),
		}));
	};

	/** The agent's session while its shell runs. */
	const sessionOf = (agent: AgentId): Session | undefined => {
		seed();
		const session = sessions.get(agent);
		if (session && find(session.terminal)?.status.state === "running") return session;
		if (session) forget(agent, session);
		return undefined;
	};

	/** Ends a block as the shell says it ended. */
	const ended = (agent: AgentId, session: Session, item: ItemId, id: number, end: BlockEnd) => {
		if (session.running?.block === id) session.running = null;
		amendBlock(agent, item, id, (block) => ({
			...block,
			process: {
				state: "exited",
				code: end.code,
				signal: null,
				...(end.stopped ? { stopped: true } : {}),
			},
			duration_ms: Math.max(0, now() - block.started_at),
			lines: end.printed.lines,
			output: end.printed.text,
			output_truncated: end.printed.truncated,
			secret_prompts: end.secrets,
			cwd_after: end.cwd === block.cwd ? null : end.cwd,
		}));
	};

	/**
	 * Adds a block that runs to the session's open card, starting one when
	 * there's none, and answers the card.
	 */
	const addBlock = (
		agent: AgentId,
		session: Session,
		terminal: Terminal,
		command: string,
		options: { readonly private: boolean; readonly typed?: boolean }
	): { readonly item: ItemId; readonly id: number } => {
		const id = session.next;
		session.next += 1;
		const block: SessionBlock = {
			id,
			command,
			cwd: stream.cwd(session.terminal) ?? terminal.cwd,
			cwd_after: null,
			private: options.private,
			process: { state: "running" },
			started_at: now(),
			duration_ms: null,
			lines: 0,
			output: "",
			output_truncated: false,
			secret_prompts: 0,
			start: null,
			...(options.typed ? { typed: true } : {}),
		};
		let item = session.card;
		if (item) {
			runs?.amend(agent, item, (card) =>
				card.kind === "user_session"
					? {
							...card,
							blocks: [...card.blocks, block],
							status: "running",
							completed_at: null,
						}
					: card
			);
		} else {
			cards += 1;
			item = `${agent}:session-${cards}`;
			session.card = item;
			const open = openCards.get(agent) ?? new Set<ItemId>();
			open.add(item);
			openCards.set(agent, open);
			runs?.post(
				agent,
				{
					kind: "user_session",
					terminal: session.terminal,
					device: terminal.device,
					blocks: [block],
					sealed: false,
					sent: false,
				},
				"running",
				item
			);
		}
		session.running = { item, block: id };
		return { item, id };
	};

	/** Runs a command in the session as a block of its open card, starting one when there's none. */
	const runIn = (
		agent: AgentId,
		session: Session,
		command: string,
		privately: boolean
	): ActionResult => {
		const terminal = find(session.terminal);
		if (!terminal) return notFound(`terminal ${session.terminal}`);
		const { item: card, id } = addBlock(agent, session, terminal, command, {
			private: privately,
		});
		const start = stream.command(session.terminal, command, (end) =>
			ended(agent, session, card, id, end)
		);
		if (start !== null) amendBlock(agent, card, id, (each) => ({ ...each, start }));
		return answer({ status: "session_ran", terminal: session.terminal, item: card, block: id });
	};

	/** Opens the agent's session in `cwd`, or its folder. */
	const openSession = (
		agent: AgentId,
		size: TerminalSize,
		cwd?: string
	): Session | ActionResult => {
		const result = hooks.open(agent, { type: "session" }, size, { cwd });
		const terminal = opened(result);
		if (!terminal) return result;
		const session: Session = { terminal, card: null, next: 1, running: null };
		sessions.set(agent, session);
		return session;
	};

	/** Why a command can't run in the agent's session, or null when it can. */
	const freshReason = (
		agent: AgentId,
		device: DeviceId,
		command: string,
		fresh: boolean
	): FreshReason | null => {
		if (fresh) return "asked";
		const reason = reasonIn(command);
		if (reason) return reason;
		if (!hooks.marks(device)) return "integration_off";
		const session = sessionOf(agent);
		if (!session) return null;
		if (find(session.terminal)?.marks === false) return "no_marks";
		return stream.idle(session.terminal) ? null : "busy";
	};

	const busy = (session: Session) => {
		const title = find(session.terminal)?.title.trim() || "a command";
		return refused(`The session is busy. Wait for ${title} to finish, or stop it.`);
	};

	return {
		sessionRun: async (agent, command, { private: privately, fresh, size }) => {
			const place = hooks.placeOf(agent);
			if (!place) return notFound(`agent ${agent}`);
			const reason = freshReason(agent, place.device, command, fresh);
			if (reason) {
				// A fresh shell starts where the session is, or where the agent works.
				const session = sessionOf(agent);
				const cwd = session ? stream.cwd(session.terminal) : undefined;
				return hooks.open(agent, { type: "run", command, private: privately }, size, {
					cwd,
					fresh: reason,
				});
			}
			const session = sessionOf(agent) ?? openSession(agent, size);
			if (!("terminal" in session)) return session;
			return runIn(agent, session, command, privately);
		},
		sessionShare: async (agent, item, block, privately) => {
			let outcome = notFound(`item ${item}`);
			runs?.amend(agent, item, (card) => {
				if (card.kind !== "user_session") return card;
				if (card.sealed) {
					outcome = refused(CARD_SEALED);
					return card;
				}
				if (!card.blocks.some((each) => each.id === block)) {
					outcome = notFound(`block ${block}`);
					return card;
				}
				outcome = DONE;
				return {
					...card,
					blocks: card.blocks.map((each) =>
						each.id === block ? { ...each, private: privately } : each
					),
				};
			});
			return outcome;
		},
		sessionCd: async (agent, path) => {
			if (!hooks.placeOf(agent)) return notFound(`agent ${agent}`);
			const session = sessionOf(agent);
			if (!session) {
				const made = openSession(agent, CD_SIZE, path);
				return "terminal" in made
					? answer({ status: "terminal_opened", terminal: made.terminal })
					: made;
			}
			if (!stream.idle(session.terminal)) return busy(session);
			return runIn(agent, session, `cd -- ${quoted(path)}`, false);
		},
		sessionEnd: async (agent, force = false) => {
			const session = sessionOf(agent);
			if (!session) return DONE;
			if (!force && find(session.terminal)?.busy) return refused(hooks.stillRunning);
			hooks.close(session.terminal);
			return DONE;
		},
		sealSessions: (agent, input) => {
			seed();
			const open = openCards.get(agent);
			if (!open || open.size === 0) return;
			const taken = new Set(
				(input.refs ?? []).flatMap((ref) =>
					ref.type === "session" && ref.item ? [ref.item] : []
				)
			);
			for (const item of open) {
				runs?.amend(agent, item, (card) =>
					card.kind === "user_session"
						? { ...card, sealed: true, sent: taken.has(item) }
						: card
				);
			}
			openCards.delete(agent);
			const session = sessions.get(agent);
			if (session) session.card = null;
		},
		sessionClosed: (terminal) => {
			seed();
			for (const [agent, session] of sessions)
				if (session.terminal === terminal) forget(agent, session);
		},
		typedIn: (terminal, line) => {
			seed();
			const found = [...sessions].find(([, session]) => session.terminal === terminal);
			const open = find(terminal);
			if (!found || !open || open.status.state !== "running") return null;
			const [agent, session] = found;
			// A typed line starts Private, so the agent sees it only once you share it.
			const { item, id } = addBlock(agent, session, open, maskedLine(line), {
				private: true,
				typed: true,
			});
			return (end) => ended(agent, session, item, id, end);
		},
	};
};
