// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A busy few seconds to replay through the app, for the CPU benchmarks.
// Several agents stream long Markdown answers with tool calls between them,
// and maxd sends each agent with its newest chunk four times a second, as
// its context grows. Only the shown agent's timeline is open, so only its
// events come.

import { Lexer } from "marked";

import type { AgentId, Change, TimelineEvent, TimelineItem, Timestamp } from "../bindings";
import { timelineItem } from "../timeline/build";
import type { EntityOf } from "../view/replica";

/** A frame, as the app core batches what it sends a window. */
export const FRAME_MS = 16;

/** How often each agent streams a chunk, as the CLIs do in a fast answer. */
const CHUNK_MS = 25;

/** How much text a chunk carries, a few tokens. */
const CHUNK_CHARS = 9;

/** How many tokens of context a chunk adds. */
const CHUNK_TOKENS = 3;

/** How often maxd sends an agent whose context grows. */
const PACE_MS = 250;

/** How many chunks a tool call takes, from its start to its result. */
const TOOL_CHUNKS = 12;

/** How many chunks of each answer come before the next tool call. */
const ANSWER_CHUNKS = 80;

/** One section of a long answer, with most of what Markdown has. */
const section = (n: number) => `## Step ${n}: the store

The store keeps each agent's items by ID, so a row finds its item in $O(1)$ and
a change to one item leaves the others where they were. Section ${n} covers the
part that moves, and \`applyEvents\` does the work.

- It reads the batch once, in order.
- It merges appends to one item before it copies the text.
- It keeps the rows, unless an item came or went.

1. Open the timeline.
2. Apply the reset.
3. Apply each batch on the next frame.

\`\`\`ts
const next = applyEvents(state, events);
if (next !== state) store.setState(next, true);
\`\`\`

| Step | Work | Cost |
| --- | --- | --- |
| Reset | Builds the map | Once |
| Batch ${n} | Copies one item | Per frame |

\`\`\`mermaid
graph TD;
  Event${n}-->Batch${n};
  Batch${n}-->Render${n};
\`\`\`

> The page renders once a frame, however many chunks came in it.

`;

/** A long answer of at least `length` characters. */
export const longAnswer = (length: number): string => {
	let answer = "";
	for (let n = 1; answer.length < length; n++) answer += section(n);
	return answer;
};

/** What maxd sends the page in one frame. */
export type ReplayFrame = {
	/** The changes of each delta, one per agent sent, in order. */
	readonly view: readonly (readonly Change[])[];
	/** The shown agent's events, as one batch. Empty when it sent none. */
	readonly events: readonly TimelineEvent[];
};

/** A replay: the shown agent's newest items, and the frames after them. */
export type Replay = {
	readonly items: readonly TimelineItem[];
	readonly frames: readonly ReplayFrame[];
};

/** What the replay streams. */
export type ReplayOptions = {
	/** The agents that stream, as the view has them. The first one is shown. */
	readonly agents: readonly EntityOf<"agent">[];
	/** How long to replay. */
	readonly seconds: number;
	/** When the replay starts. */
	readonly now: Timestamp;
	/** How long the answer streaming at the start is so far. */
	readonly streamed: number;
	/** How many items came before it. */
	readonly history: number;
};

/** The shown agent's items before the replay: turns of a message, a command and an answer. */
const historyOf = (agent: AgentId, count: number, answer: string): TimelineItem[] =>
	Array.from({ length: count }, (_, n) => {
		const head = { id: `h${n}`, agent, ordinal: n };
		switch (n % 3) {
			case 0:
				return timelineItem(
					{
						kind: "user_message",
						text: `Do step ${n}.`,
						attachments: [],
						delivery: "sent",
					},
					head
				);
			case 1:
				return timelineItem(
					{
						kind: "command",
						command: `cargo test step_${n}`,
						cwd: null,
						exit_code: 0,
						output: "test result: ok. 12 passed\n",
						output_truncated: false,
						background: false,
					},
					head
				);
			default:
				return timelineItem(
					{ kind: "assistant_message", text: answer.slice(0, 1500), streaming: false },
					head
				);
		}
	});

/**
 * Builds the replay. Each agent streams a chunk every 25 ms, the agents a
 * few ms apart, and goes out with its newest chunk every 250 ms. The shown
 * agent's chunks also come as timeline events: text for its answer, then a
 * command and its output, then a new answer.
 */
export const replay = (options: ReplayOptions): Replay => {
	const { agents, seconds, now, streamed, history } = options;
	const shown = agents[0];
	if (!shown) throw new Error("A replay needs an agent to show");
	const answer = longAnswer(streamed + Math.ceil((seconds * 1000) / CHUNK_MS) * CHUNK_CHARS);
	const items = historyOf(shown.id, history, answer);
	let ordinal = history;
	let message = timelineItem(
		{ kind: "assistant_message", text: answer.slice(0, streamed), streaming: true },
		{ id: `m${ordinal}`, agent: shown.id, ordinal: ordinal++, status: "running" }
	);
	items.push(message);
	let at = streamed;
	let chunks = 0;
	let tool: TimelineItem | null = null;

	// The shown agent's next chunk, as the events it brings.
	const step = (time: Timestamp): TimelineEvent[] => {
		chunks++;
		if (tool) {
			const done = chunks % (ANSWER_CHUNKS + TOOL_CHUNKS) === 0;
			if (!done)
				return [{ type: "append", id: tool.id, field: "output", text: `line ${chunks}\n` }];
			const finished = {
				...tool,
				status: "completed" as const,
				completed_at: time,
				exit_code: 0,
			};
			tool = null;
			message = timelineItem(
				{ kind: "assistant_message", text: "", streaming: true },
				{ id: `m${ordinal}`, agent: shown.id, ordinal: ordinal++, status: "running" }
			);
			return [
				{ type: "upsert", item: finished },
				{ type: "upsert", item: message },
			];
		}
		if (chunks % (ANSWER_CHUNKS + TOOL_CHUNKS) === ANSWER_CHUNKS) {
			const ended = { ...message, status: "completed" as const, streaming: false };
			tool = timelineItem(
				{
					kind: "command",
					command: "pnpm test",
					cwd: null,
					exit_code: null,
					output: "",
					output_truncated: false,
					background: false,
				},
				{
					id: `c${ordinal}`,
					agent: shown.id,
					ordinal: ordinal++,
					status: "running",
					started_at: time,
				}
			);
			return [
				{ type: "upsert", item: ended as TimelineItem },
				{ type: "upsert", item: tool },
			];
		}
		const text = answer.slice(at, at + CHUNK_CHARS);
		at += CHUNK_CHARS;
		if (message.kind === "assistant_message")
			message = { ...message, text: message.text + text };
		return [{ type: "append", id: message.id, field: "text", text }];
	};

	// The agent as maxd sends it after the chunk at `time`.
	const sent = (agent: EntityOf<"agent">, time: number): EntityOf<"agent"> => ({
		...agent,
		last_activity_at: now + time,
		context: agent.context && {
			...agent.context,
			used_tokens: agent.context.used_tokens + Math.floor(time / CHUNK_MS) * CHUNK_TOKENS,
		},
	});
	const sentAt = agents.map(() => -PACE_MS);
	const frames: ReplayFrame[] = [];
	const count = Math.round((seconds * 1000) / FRAME_MS);
	for (let frame = 0; frame < count; frame++) {
		const start = frame * FRAME_MS;
		const view: Change[][] = [];
		const events: TimelineEvent[] = [];
		// Every chunk that falls in this frame, in time order across agents.
		const due: { time: number; index: number }[] = [];
		agents.forEach((_, index) => {
			const offset = (index * 7) % CHUNK_MS;
			const first = Math.ceil((start - offset) / CHUNK_MS) * CHUNK_MS + offset;
			for (let time = first; time < start + FRAME_MS; time += CHUNK_MS) {
				if (time >= start) due.push({ time, index });
			}
		});
		due.sort((a, b) => a.time - b.time || a.index - b.index);
		for (const { time, index } of due) {
			const agent = agents[index];
			if (!agent) continue;
			if (time - (sentAt[index] ?? 0) >= PACE_MS) {
				sentAt[index] = time;
				view.push([{ op: "upsert", value: sent(agent, time) }]);
			}
			if (index === 0) events.push(...step(now + time));
		}
		frames.push({ view, events });
	}
	return { items, frames };
};

/** How much Markdown went through the lexer. */
export type Lexing = { calls: number; chars: number };

/**
 * Counts the texts the lexer splits into blocks until `stop` is called. The
 * Markdown split goes through it, so this measures that work. A block inside
 * a block, such as a list item, is part of its text, so it doesn't count again.
 */
export const countLexing = (): { readonly lexing: Lexing; readonly stop: () => void } => {
	const lexer = Lexer.prototype;
	const blockTokens = lexer.blockTokens;
	const lexing: Lexing = { calls: 0, chars: 0 };
	let depth = 0;
	const counted = function (this: Lexer, src: string, ...rest: unknown[]) {
		if (depth === 0) {
			lexing.calls++;
			lexing.chars += src.length;
		}
		depth++;
		try {
			return (blockTokens as (...args: unknown[]) => unknown).call(this, src, ...rest);
		} finally {
			depth--;
		}
	};
	lexer.blockTokens = counted as typeof blockTokens;
	return { lexing, stop: () => void (lexer.blockTokens = blockTokens) };
};
