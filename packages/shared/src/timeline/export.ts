// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A conversation as one Markdown document, for an export. It keeps what
// you sent, the replies, each tool step as a line, each sub-agent's task
// with its report, plans, files the agent sent and moves to another vendor.
// Thinking, approvals, questions and the timeline's notices are left out.
// A heading starts each change of speaker, and the agent's heading names
// the vendor it ran on at that point. The file it's saved in is named
// after the agent and the day.

import type { AttachmentId, TimelineItem } from "../bindings";
import type { ToolNames } from "../agent-view/agent-tools";
import { handoffLine } from "../agent-view/format";
import { inlineCode, toolLines } from "../agent-view/tool-line";
import { excerptIds } from "../pills/draft";
import { sentPieces, sentText } from "../pills/sent";
import { sentence } from "../status/format";
import { kindName, type AgentKind } from "../status/agent-kind";

/** What the export needs beyond the items. */
export type ExportOptions = {
	/** The agent's title, which heads the document. */
	readonly title?: string;
	/** What the agent runs as now: its vendor, or Local. */
	readonly vendor: AgentKind;
	/** Names the agents and devices an agent tool acted on. */
	readonly names?: ToolNames;
};

const NO_NAMES: ToolNames = { agent: () => undefined, device: (id) => id };

// Text as a Markdown quote, each line marked.
const quoted = (text: string): string =>
	text
		.trim()
		.split("\n")
		.map((line) => (line.trim() === "" ? ">" : `> ${line}`))
		.join("\n");

const oneLine = (text: string): string => text.replace(/\s+/g, " ").trim();

/**
 * What you sent, with the files that weren't placed in the text named at
 * the end. A placed file shows as its marker, such as `[Image 1: shot.png]`.
 */
export const userText = (item: Extract<TimelineItem, { kind: "user_message" }>): string => {
	const files = new Map(item.attachments.map((file) => [file.id, file]));
	const pieces = sentPieces(item);
	const text = sentText(pieces, (id: AttachmentId) => files.get(id)).trim();
	const placed = new Set(
		pieces.flatMap((piece) =>
			piece.type === "pill" && piece.ref.type === "attachment" ? [piece.ref.id] : []
		)
	);
	const excerpts = excerptIds(item.refs);
	const rest = item.attachments.filter((file) => !placed.has(file.id) && !excerpts.has(file.id));
	const attached = rest.length > 0 ? `Attached: ${rest.map((file) => file.name).join(", ")}` : "";
	return [text, attached].filter(Boolean).join("\n\n");
};

/**
 * The conversation in `items`, in the order given, as Markdown. Messages
 * still queued or cancelled are skipped, and so are the steps a sub-agent
 * took under its card, since its report stands for them.
 */
export const conversationMarkdown = (
	items: readonly TimelineItem[],
	options: ExportOptions
): string => {
	const names = options.names ?? NO_NAMES;
	const subAgents = new Set(items.flatMap((item) => (item.kind === "subagent" ? [item.id] : [])));
	const firstMove = items.find((item) => item.kind === "handoff");
	let vendor: AgentKind = firstMove?.kind === "handoff" ? firstMove.from_vendor : options.vendor;

	const blocks: string[] = options.title ? [`# ${oneLine(options.title)}`] : [];
	let speaker: string | null = null;
	let steps: string[] = [];

	const flushSteps = () => {
		if (steps.length === 0) return;
		blocks.push(steps.map((line) => `- ${line}`).join("\n"));
		steps = [];
	};
	// A heading when the speaker changes, after the last speaker's steps.
	const heading = (who: string) => {
		if (who === speaker) return;
		flushSteps();
		blocks.push(`## ${who}`);
		speaker = who;
	};
	const say = (who: string, block: string) => {
		heading(who);
		flushSteps();
		blocks.push(block);
	};
	const agentSays = (block: string) => say(kindName(vendor), block);

	for (const item of items) {
		if (item.parent !== null && subAgents.has(item.parent)) continue;
		switch (item.kind) {
			case "user_message": {
				if (item.delivery === "queued" || item.delivery === "cancelled") break;
				const text = userText(item);
				if (text) say("You", text);
				break;
			}
			case "assistant_message":
				if (item.text.trim()) agentSays(item.text.trim());
				break;
			case "agent_attachments": {
				const sent =
					item.attachments.length > 0
						? `Sent: ${item.attachments.map((file) => file.name).join(", ")}`
						: "";
				const block = [item.text.trim(), sent].filter(Boolean).join("\n\n");
				if (block) agentSays(block);
				break;
			}
			case "plan":
				if (item.markdown.trim()) agentSays(`**Plan**\n\n${item.markdown.trim()}`);
				break;
			case "subagent": {
				const kind = item.agent_type ? sentence(item.agent_type) : "Task";
				const task = `**Sub-agent, ${kind}:** ${oneLine(item.description)}`;
				agentSays(item.result?.trim() ? `${task}\n\n${quoted(item.result)}` : task);
				break;
			}
			case "error":
				if (!item.retrying) agentSays(`*Error: ${oneLine(item.message)}*`);
				break;
			case "agent_opened_file": {
				heading(kindName(vendor));
				const at = item.line != null ? ` at line ${item.line}` : "";
				steps.push(`Opened ${inlineCode(item.path)}${at} for you`);
				break;
			}
			case "agent_visual":
				agentSays(
					`**Visual:** ${oneLine(item.title)} (${inlineCode(item.attachment.name)})`
				);
				break;
			case "handoff":
				flushSteps();
				blocks.push(`*${handoffLine(item)}*`);
				if (item.status !== "cancelled") vendor = item.to_vendor;
				break;
			default: {
				const lines = toolLines(item, names);
				if (lines.length === 0) break;
				heading(kindName(vendor));
				steps.push(...lines);
			}
		}
	}
	flushSteps();
	return `${blocks.join("\n\n")}\n`;
};

/** The most of a title an export's file name keeps, in characters. */
const NAME_TITLE_CHARS = 80;

/** Characters a file name can't hold on one system or another. */
const NOT_IN_NAMES = '/\\:*?"<>|';

const twoDigits = (value: number): string => String(value).padStart(2, "0");

/**
 * The file name an export suggests: the agent's title and the day on this
 * device, such as `Fix the login bug 2026-11-08.md`. A character a file name
 * can't hold becomes a space, a long title is cut, and a title with nothing
 * left reads "Conversation".
 */
export const exportFileName = (title: string | undefined, at: Date): string => {
	const safe = [...(title ?? "")]
		.map((char) =>
			char < " " || char === "\u007f" || NOT_IN_NAMES.includes(char) ? " " : char
		)
		.join("");
	// A leading dot would hide the file.
	const words = [...oneLine(safe).replace(/^[.\s]+/, "")]
		.slice(0, NAME_TITLE_CHARS)
		.join("")
		.trim();
	const day = `${at.getFullYear()}-${twoDigits(at.getMonth() + 1)}-${twoDigits(at.getDate())}`;
	return `${words || "Conversation"} ${day}.md`;
};
