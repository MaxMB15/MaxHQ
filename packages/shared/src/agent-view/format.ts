// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Short forms the timeline rows use: how long a step took, file names,
// sizes, line counts, what a tool step acts on and where an agent moved.

import { t, formatNumber } from "../i18n";

import type { AttachmentRef, ItemStatus, TimelineItem } from "../bindings";
import { formatDuration, plural } from "../status/format";
import { localAgentLabel } from "../local-models/wording";
import { kindName, type AgentKind } from "../status/agent-kind";
import { artifactLine } from "./artifact-line";

/** Whether an item's status means it's still going. */
export const isOpen = (status: ItemStatus): boolean =>
	status === "pending" || status === "running" || status === "waiting";

/** What a step that didn't finish says at the end of its row, or null for any other. */
export const endedEarly = (status: ItemStatus): string | null => {
	switch (status) {
		case "declined":
			return t("Declined");
		case "cancelled":
		case "interrupted":
			return t("Stopped");
		case "failed":
			return t("Failed");
		default:
			return null;
	}
};

/** How long a step took, in tenths under ten seconds, such as "7.1 s", then in words. */
export const formatTook = (ms: number): string =>
	ms < 10_000
		? t("{{seconds}} s", {
				seconds: formatNumber(Math.max(0, ms) / 1000, {
					minimumFractionDigits: 1,
					maximumFractionDigits: 1,
				}),
			})
		: formatDuration(ms);

/** How long an item ran, once it has ended and the CLI said when. */
export const tookOf = (item: Pick<TimelineItem, "started_at" | "completed_at">): number | null =>
	item.started_at !== null && item.completed_at !== null
		? item.completed_at - item.started_at
		: null;

/** What an agent's files say with no message of their own, such as "Sent you 2 files". */
export const sentFilesLine = (count: number): string =>
	count === 1 ? t("Sent you a file") : t("Sent you {{count}} files", { count: count });

/** The last part of a path, such as "header.tsx". */
export const baseName = (path: string): string => path.split("/").filter(Boolean).at(-1) ?? path;

/**
 * What a file the agent opened for you reads as, such as "Opened plan.md at
 * line 12". It names the file alone, so it suits a phone too.
 */
export const openedLine = (item: {
	readonly path: string;
	readonly line?: number | null;
}): string =>
	t("Opened {{baseName_item_path}}{{item_line____null_____at_line___item_line}}", {
		baseName_item_path: baseName(item.path),
		item_line____null_____at_line___item_line:
			item.line != null ? t(" at line {{value1}}", { value1: item.line }) : "",
	});

/** What a visual the agent showed reads as, such as "Showed Weekly usage". */
export const visualLine = (item: { readonly title: string }): string =>
	t("Showed {{title}}", { title: item.title.trim() || t("a visual") });

/** The folder a path is in, or "/" for a file at the root. */
export const folderOf = (path: string): string => {
	const at = path.replace(/\/+$/, "").lastIndexOf("/");
	return at > 0 ? path.slice(0, at) : "/";
};

/** A line range in words, such as "31 lines" for "10-40" or "Line 12". */
export const lineSpan = (lines: string | null): string | null => {
	if (lines === null) return null;
	const range = /^(\d+)\s*-\s*(\d+)$/.exec(lines.trim());
	if (range) {
		const count = Number(range[2]) - Number(range[1]) + 1;
		return count > 0 ? plural(count, "line") : null;
	}
	return /^\d+$/.test(lines.trim())
		? t("Line {{lines_trim}}", { lines_trim: lines.trim() })
		: lines;
};

/** A size on disk, such as "220 KB" or "1.4 MB". */
export const formatBytes = (bytes: number): string => {
	if (bytes < 1000) return t("{{value}} B", { value: formatNumber(bytes) });
	if (bytes < 1_000_000)
		return t("{{value}} KB", { value: formatNumber(Math.round(bytes / 1000)) });
	const mb = bytes / 1_000_000;
	return t("{{value}} MB", {
		value: formatNumber(mb, {
			minimumFractionDigits: mb < 10 ? 1 : 0,
			maximumFractionDigits: mb < 10 ? 1 : 0,
		}),
	});
};

/** What an attachment is, with its size, such as "PNG, 44 KB" or "Folder". */
export const attachmentMeta = (file: AttachmentRef): string => {
	const ext = /\.([a-z0-9]{1,6})$/i.exec(file.name)?.[1];
	const kind =
		file.kind === "folder"
			? t("Folder")
			: file.kind === "pdf"
				? "PDF"
				: file.kind === "image"
					? (ext?.toUpperCase() ?? t("Image"))
					: ext
						? ext[0]!.toUpperCase() + ext.slice(1).toLowerCase()
						: t("File");
	return file.kind === "folder" ? kind : `${kind}, ${formatBytes(file.bytes)}`;
};

/** How many lines a block of text has, not counting a final newline. */
export const lineCount = (text: string): number => {
	if (text === "") return 0;
	const lines = text.split("\n").length;
	return text.endsWith("\n") ? lines - 1 : lines;
};

/** The kinds that are tool steps, each drawn as one row. */
export const TOOL_KINDS: ReadonlySet<TimelineItem["kind"]> = new Set([
	"command",
	"file_change",
	"file_read",
	"search",
	"web",
	"tool_call",
]);

type Kind = TimelineItem["kind"];

/**
 * The space above an item, in pixels, from its kind and the kind before it.
 * Tool steps sit tight together, a reply further from what came before it.
 */
export const gapAbove = (kind: Kind, before: Kind | null): number => {
	if (before === null) return 0;
	if (TOOL_KINDS.has(kind) && TOOL_KINDS.has(before)) return 1;
	if (TOOL_KINDS.has(kind) && before === "reasoning") return 2;
	if (before === "user_message") return 14;
	return 10;
};

/**
 * What a tool step acts on, in one line: a command's first line, a path, a
 * query, a claude.ai page or a tool's name. Null for anything that isn't a
 * tool step.
 */
export const toolTarget = (item: TimelineItem): string | null => {
	switch (item.kind) {
		case "command":
			return item.command.split("\n")[0] ?? item.command;
		case "file_change":
			return item.changes[0]?.path ?? null;
		case "file_read":
			return item.path;
		case "search":
			return item.scope
				? t("{{value1}} in {{value2}}", { value1: item.query, value2: item.scope })
				: item.query;
		case "web":
			return item.target;
		case "tool_call":
			return (
				artifactLine(item)?.target ||
				(item.server ? `${item.server}:${item.name}` : item.name)
			);
		default:
			return null;
	}
};

/**
 * What the marker of a move to another vendor says. It waits for the next
 * message to carry the conversation over, and is skipped when the agent
 * moves again first.
 */
export const handoffTitle = (status: ItemStatus): string =>
	status === "pending"
		? t("Context handoff pending")
		: status === "cancelled"
			? t("Context handoff skipped")
			: t("Context handoff");

/**
 * One end of a move: the kind, such as Codex or Local, then the model when
 * one was set. A local end reads as a local agent does everywhere:
 * "Local · <model>".
 */
export const handoffEnd = (kind: AgentKind, model: string | null): string =>
	kind === "local"
		? localAgentLabel(model)
		: model
			? `${kindName(kind)} ${model}`
			: kindName(kind);

/**
 * The kinds a move went from and to: Local for an end on a local model,
 * which runs as Claude, else the vendor.
 */
export const handoffKinds = (
	item: Pick<
		Extract<TimelineItem, { kind: "handoff" }>,
		"from_vendor" | "to_vendor" | "from_local" | "to_local"
	>
): { readonly from: AgentKind; readonly to: AgentKind } => ({
	from: item.from_local ? "local" : item.from_vendor,
	to: item.to_local ? "local" : item.to_vendor,
});

/** A move's marker as one line, such as "Context handoff, Claude opus to Codex". */
export const handoffLine = (item: Extract<TimelineItem, { kind: "handoff" }>): string => {
	const kinds = handoffKinds(item);
	return t("{{value1}}, {{value2}} to {{value3}}", {
		value1: handoffTitle(item.status),
		value2: handoffEnd(kinds.from, item.from_model),
		value3: handoffEnd(kinds.to, item.to_model),
	});
};

/** Where one stands among `count`, counting from one, such as "2 / 5". */
export const placeLabel = (index: number, count: number): string =>
	t("{{at}} / {{total}}", { at: index + 1, total: count });
