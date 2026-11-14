// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A quote pill made from a timeline row. The row's kind says what the
// quote came from, which maxd puts in the context entry: a reply, your own
// message, a question, or what a command or tool printed.

import type { AgentId, ItemBody, TimelineItem } from "../bindings";

import type { QuoteAnchor } from "./anchor";
import { isPill } from "./clipboard";
import type { QuoteRef } from "./draft";
import type { QuoteSource } from "./wire";

/** The clipboard and drag type that carries one quote, as JSON. */
export const QUOTE_MIME = "application/x-maxhq-quote";

/** The longest command a quote names as what printed it. */
const FROM_CHARS = 80;

const commandName = (command: string): string => {
	const line = command.trim().split("\n")[0]!.trim();
	return line.length <= FROM_CHARS ? line : `${line.slice(0, FROM_CHARS - 1).trimEnd()}…`;
};

/** What a quote from a row of this kind came from, and what printed it. */
export const quoteSource = (
	body: ItemBody
): { readonly source: QuoteSource; readonly from: string | null } => {
	switch (body.kind) {
		case "assistant_message":
		case "agent_attachments":
			return { source: "reply", from: null };
		case "user_message":
			return { source: "request", from: null };
		case "question":
			return { source: "question", from: null };
		case "command":
			return { source: "output", from: commandName(body.command) || null };
		case "tool_call":
			return { source: "output", from: body.name || null };
		case "subagent":
			return { source: "output", from: body.agent_type };
		default:
			return { source: "other", from: null };
	}
};

/** A quote of a passage in one of an agent's rows. */
export const makeQuote = (
	agent: { readonly id: AgentId; readonly title: string },
	item: TimelineItem,
	anchor: QuoteAnchor
): QuoteRef => ({
	type: "quote",
	agent: agent.id,
	agent_title: agent.title,
	item: item.id,
	text: anchor.text,
	position: anchor.position,
	...quoteSource(item),
});

const SOURCES: ReadonlySet<string> = new Set<QuoteSource>([
	"reply",
	"request",
	"question",
	"output",
	"other",
]);

/** The quote in a drag's or a clipboard's JSON, or null when it holds none. */
export const parseQuote = (raw: string | null | undefined): QuoteRef | null => {
	if (!raw) return null;
	let value: unknown;
	try {
		value = JSON.parse(raw);
	} catch {
		return null;
	}
	return isPill(value) && value.type === "quote" && SOURCES.has(value.source) ? value : null;
};
