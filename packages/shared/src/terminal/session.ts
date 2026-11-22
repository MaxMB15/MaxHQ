// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// An agent's shell session: the one shell the commands you run with `!`
// share, so a `cd` or an `export` holds for the next one. Its requests run
// a command there, keep one from the agent, move it to a folder and end
// it. Between two of your messages its commands make one card, and while
// that card has a command the agent may see, one pill in the composer
// stands for it. When the message goes, the pill takes the shared
// commands' last lines, 200 each and 600 in all, the oldest giving up
// lines first.

import {
	type AgentId,
	type DeviceId,
	type FreshReason,
	type ItemId,
	SESSIONS_SINCE,
	type SessionBlock,
	type SessionRefBlock,
	type TerminalId,
	type TimelineItem,
	type Timestamp,
} from "../bindings";
import type { Ask } from "../data/request-source";
import type { ActionResult } from "../data/source";
import { t } from "../i18n";
import type { SessionRef } from "../pills/draft";
import type { TimelineState } from "../timeline/apply";
import { stripAnsi } from "../timeline/ansi";
import { all, type ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

import { lastLines } from "./output";
import type { TerminalSize } from "./requests";
import { supportsTerminals, type TerminalEntity } from "./select";

/** A card of commands you ran in the session, as the timeline holds it. */
export type UserSessionItem = Extract<TimelineItem, { kind: "user_session" }>;

/** What a timeline holds that the session reads. */
type Items = Pick<TimelineState, "items" | "order">;

// Requests -------------------------------------------------------------------

/** How a command runs from the composer. */
export type SessionRunOptions = {
	/** Typed after `!!`, so its block starts private. */
	readonly private: boolean;
	/** In a fresh shell rather than the session, from ⌥↩. */
	readonly fresh: boolean;
	/** The size of a terminal it opens. */
	readonly size: TerminalSize;
};

/** The session calls of a data source. */
export type SessionCalls = {
	/**
	 * Runs a command typed after `!` in the agent's session, opening it
	 * first when there's none. Answers `session_ran` for a block in the
	 * session's card, or `terminal_opened` when it ran in a fresh shell.
	 */
	readonly sessionRun: (
		agent: AgentId,
		command: string,
		options: SessionRunOptions
	) => Promise<ActionResult>;
	/** Keeps one block of an open card from the agent, or shares it again. */
	readonly sessionShare: (
		agent: AgentId,
		item: ItemId,
		block: number,
		privately: boolean
	) => Promise<ActionResult>;
	/** Moves the session to a folder, or opens it there when there's none. */
	readonly sessionCd: (agent: AgentId, path: string) => Promise<ActionResult>;
	/** Ends the session, killing a command that still runs only with `force`. */
	readonly sessionEnd: (agent: AgentId, force?: boolean) => Promise<ActionResult>;
};

/** The session calls, each one request over `ask`. */
export const sessionCalls = (ask: Ask): SessionCalls => ({
	sessionRun: (agent, command, { private: privately, fresh, size }) =>
		ask({
			kind: "session_run",
			agent,
			command,
			private: privately,
			fresh,
			cols: size.cols,
			rows: size.rows,
		}),
	sessionShare: (agent, item, block, privately) =>
		ask({ kind: "session_share", agent, item, block, private: privately }),
	sessionCd: (agent, path) => ask({ kind: "session_cd", agent, path }),
	sessionEnd: (agent, force = false) => ask({ kind: "session_end", agent, force }),
});

/** Where a command that ran in the session went: its block in a card. */
export type SessionRan = {
	readonly terminal: TerminalId;
	readonly item: ItemId;
	readonly block: number;
};

/** The block a run answered with, or null when it didn't make one. */
export const sessionRanOf = (result: ActionResult): SessionRan | null =>
	result.status === "ok" && result.data.status === "session_ran"
		? { terminal: result.data.terminal, item: result.data.item, block: result.data.block }
		: null;

// The view ---------------------------------------------------------------------

/**
 * Whether the device's `maxd` keeps a session for `!` commands: it has
 * terminals, and speaks protocol 48 or later. An older one runs each
 * command in a shell of its own.
 */
export const supportsSessions = (view: ViewState, device: DeviceId): boolean => {
	if (!supportsTerminals(view, device)) return false;
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	return protocol !== null && protocol >= SESSIONS_SINCE;
};

/** The agent's session terminal while it runs, if it has one. */
export const sessionTerminalOf = (view: ViewState, agent: AgentId): TerminalEntity | undefined =>
	all(view, "terminal")
		.filter(
			(terminal) =>
				terminal.agent === agent &&
				terminal.kind.type === "session" &&
				terminal.status.state === "running"
		)
		.sort((a, b) => b.started_at - a.started_at)[0];

// Cards -------------------------------------------------------------------------

/** The card a message hasn't closed yet, the one the next command goes in. */
export const openCardOf = (state: Items): UserSessionItem | undefined => {
	for (let index = state.order.length - 1; index >= 0; index -= 1) {
		const item = state.items.get(state.order[index]!);
		if (item?.kind === "user_session" && !item.sealed) return item;
	}
	return undefined;
};

/** The card with this ID, if the timeline holds it. */
export const sessionCardOf = (state: Items, id: ItemId): UserSessionItem | undefined => {
	const item = state.items.get(id);
	return item?.kind === "user_session" ? item : undefined;
};

/**
 * The folder the session is in: where its last command left it, or where
 * it started. Null with no session.
 */
export const sessionFolder = (
	terminal: Pick<TerminalEntity, "id" | "cwd"> | undefined,
	state: Items
): string | null => {
	if (!terminal) return null;
	for (let index = state.order.length - 1; index >= 0; index -= 1) {
		const item = state.items.get(state.order[index]!);
		if (item?.kind !== "user_session" || item.terminal !== terminal.id) continue;
		const last = item.blocks.at(-1);
		if (last) return last.cwd_after ?? last.cwd;
	}
	return terminal.cwd;
};

/** Whether a block still runs. */
export const blockRunning = (block: Pick<SessionBlock, "process">): boolean =>
	block.process.state === "running";

/** Whether a block ended with an exit code other than 0. */
export const blockFailed = (block: Pick<SessionBlock, "process">): boolean =>
	block.process.state === "exited" && block.process.code !== null && block.process.code !== 0;

/** How a card stands: open, or closed by a message that took it along or didn't. */
export type CardState = "open" | "sent" | "not_sent";

export const cardState = (item: Pick<UserSessionItem, "sealed" | "sent">): CardState =>
	!item.sealed ? "open" : item.sent ? "sent" : "not_sent";

/** What a card says of its blocks. */
export type CardSummary = {
	readonly total: number;
	/** The blocks the agent may see. */
	readonly shared: number;
	/** The shared blocks that failed. */
	readonly failed: number;
	/** Whether a shared block still runs. */
	readonly running: boolean;
	readonly state: CardState;
};

export const cardSummary = (item: Pick<UserSessionItem, "blocks" | "sealed" | "sent">) => {
	const shared = item.blocks.filter((block) => !block.private);
	return {
		total: item.blocks.length,
		shared: shared.length,
		failed: shared.filter(blockFailed).length,
		running: shared.some(blockRunning),
		state: cardState(item),
	} satisfies CardSummary;
};

/** Whether a card's pill belongs in the composer: it's open and holds a shared block. */
export const wantsPill = (item: Pick<UserSessionItem, "blocks" | "sealed">): boolean =>
	!item.sealed && item.blocks.some((block) => !block.private);

/** The pill's look: red when a shared block failed, a spinner while one runs. */
export type SessionPillLook = {
	readonly total: number;
	readonly shared: number;
	readonly failed: number;
	readonly running: boolean;
	readonly tone: "normal" | "failed";
};

export const sessionPillLook = (
	item: Pick<UserSessionItem, "blocks" | "sealed" | "sent">
): SessionPillLook => {
	const summary = cardSummary(item);
	return {
		total: summary.total,
		shared: summary.shared,
		failed: summary.failed,
		running: summary.running,
		tone: summary.failed > 0 ? "failed" : "normal",
	};
};

/** The pill's words, such as "Agent shell · 1 failed". */
export const sessionPillLabel = (look: SessionPillLook): string =>
	look.failed > 0
		? t("Agent shell · {{failed}} failed", { failed: look.failed })
		: t("Agent shell");

// Sending -----------------------------------------------------------------------

/** How many of one block's last lines go to the agent. */
export const SESSION_BLOCK_LINES = 200;

/** How many lines a session's blocks take in all. */
export const SESSION_LINES = 600;

/**
 * How many lines each block takes, oldest first: its last `each` at most,
 * and when they come to more than `total`, the oldest give up lines first.
 */
export const budgetLines = (
	available: readonly number[],
	each = SESSION_BLOCK_LINES,
	total = SESSION_LINES
): number[] => {
	const taken = available.map((lines) => Math.max(0, Math.min(lines, each)));
	let over = taken.reduce((sum, lines) => sum + lines, 0) - total;
	for (let index = 0; over > 0 && index < taken.length; index += 1) {
		const given = Math.min(taken[index]!, over);
		taken[index]! -= given;
		over -= given;
	}
	return taken;
};

/** The pill a card puts in the composer. Its blocks fill in when the message goes. */
export const sessionPill = (item: Pick<UserSessionItem, "id">, deviceName: string): SessionRef => ({
	type: "session",
	device_name: deviceName,
	blocks: [],
	item: item.id,
});

/** A block's text so far, while this window shows it live, and how many lines that is. */
export type LiveText = (
	block: SessionBlock
) => { readonly text: string; readonly total: number } | null;

/**
 * The pill as the message takes it: the card's shared blocks in order,
 * each with its last lines as plain text within the budget. A block that
 * still runs goes with what it wrote so far, from the live terminal when
 * this window shows it, and says it runs.
 */
export const fillSession = (
	ref: SessionRef,
	item: UserSessionItem,
	{ now, live }: { readonly now: Timestamp; readonly live?: LiveText }
): SessionRef => {
	const shared = item.blocks.filter((block) => !block.private);
	const texts = shared.map((block) => {
		const running = blockRunning(block);
		const shown = running ? (live?.(block) ?? null) : null;
		const plain = shown ? shown.text : stripAnsi(block.output);
		const { text, total } = lastLines(plain, Number.MAX_SAFE_INTEGER);
		return { text, total: shown ? Math.max(shown.total, total) : Math.max(block.lines, total) };
	});
	const budget = budgetLines(
		texts.map(({ text }) => (text === "" ? 0 : text.split("\n").length))
	);
	const blocks = shared.map((block, index): SessionRefBlock => {
		const running = blockRunning(block);
		const { text, total } = texts[index]!;
		const take = budget[index]!;
		return {
			command: block.command,
			cwd: block.cwd,
			cwd_after: block.cwd_after,
			exit_code: block.process.state === "exited" ? block.process.code : null,
			...(block.process.state === "exited" && block.process.stopped === true
				? { stopped: true }
				: {}),
			running,
			duration_ms: running ? Math.max(0, now - block.started_at) : block.duration_ms,
			lines_total: total,
			text: take === 0 ? "" : text.split("\n").slice(-take).join("\n"),
		};
	});
	return { ...ref, blocks, item: item.id };
};

// Fresh shells --------------------------------------------------------------------

/** What a `UserRun` card says of why it ran in a fresh shell. */
export type FreshNote = {
	/**
	 * The sentence's translation key. With `setting`, `{{setting}}` is
	 * where the Shell integration setting's link goes, and `{{device}}`
	 * names the device.
	 */
	readonly key: string;
	/** Whether it names the Shell integration setting, which the link opens. */
	readonly setting: boolean;
};

/**
 * Why a command ran in a fresh shell, in one line, or null when you asked
 * for one with ⌥↩ and need no reason. With no marks there's no session,
 * and the line says what to change. A command the session couldn't take
 * says why quietly.
 */
export const freshNote = (reason: FreshReason | null | undefined): FreshNote | null => {
	switch (reason) {
		case "integration_off":
			return { key: "Sessions need {{setting}}, which is off on {{device}}.", setting: true };
		case "no_marks":
			return {
				key: "Your shell didn't report its commands, so this ran on its own.",
				setting: false,
			};
		case "busy":
			return { key: "The session was busy, so this ran on its own.", setting: false };
		case "lines":
			return { key: "This spans more than one line, so it ran on its own.", setting: false };
		case "exec":
			return { key: "This starts with exec, so it ran on its own.", setting: false };
		default:
			return null;
	}
};
