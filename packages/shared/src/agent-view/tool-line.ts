// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The verbs a tool step's row starts with, and each tool step as one line
// of Markdown for an export, such as "Ran `pnpm test`". The rows and the
// export use the same words.

import { t } from "../i18n";

import type { FileEdit, ItemStatus, TimelineItem, WebAction } from "../bindings";
import { agentToolLine, type ToolNames } from "./agent-tools";
import { artifactLine, type ArtifactLine } from "./artifact-line";

/**
 * A step's verbs: what it does while it runs, what it did once it ended,
 * and what it says while it waits for you or after you turned it down.
 */
export type Verbs = {
	readonly doing: string;
	readonly done: string;
	readonly waiting?: string;
	readonly declined?: string;
};

/** The verb for a step's status. */
export const verbFor = (status: ItemStatus, words: Verbs): string => {
	if (status === "waiting") return words.waiting ?? words.doing;
	if (status === "pending" || status === "running") return words.doing;
	if (status === "declined") return words.declined ?? words.done;
	return words.done;
};

/** The verbs for each kind of tool step but file edits and agent tools. */
export const TOOL_VERBS = {
	command: {
		get doing() {
			return t("Running");
		},
		get done() {
			return t("Ran");
		},
		get waiting() {
			return t("Wants to run");
		},
		get declined() {
			return t("Didn't run");
		},
	},
	read: {
		get doing() {
			return t("Reading");
		},
		get done() {
			return t("Read");
		},
	},
	search: {
		get doing() {
			return t("Searching for");
		},
		get done() {
			return t("Searched for");
		},
	},
	call: {
		get doing() {
			return t("Using");
		},
		get done() {
			return t("Used");
		},
	},
} as const satisfies Record<string, Verbs>;

/** The verbs for a web step, by what it did. */
export const WEB_VERBS: Record<WebAction, Verbs> = {
	search: {
		get doing() {
			return t("Searching the web for");
		},
		get done() {
			return t("Searched the web for");
		},
	},
	fetch: {
		get doing() {
			return t("Fetching");
		},
		get done() {
			return t("Fetched");
		},
	},
};

/** The verbs for each kind of file edit. */
export const EDIT_VERBS: Record<FileEdit["op"], Verbs> = {
	add: {
		get doing() {
			return t("Creating");
		},
		get done() {
			return t("Created");
		},
		get waiting() {
			return t("Wants to create");
		},
		get declined() {
			return t("Didn't create");
		},
	},
	modify: {
		get doing() {
			return t("Editing");
		},
		get done() {
			return t("Edited");
		},
		get waiting() {
			return t("Wants to edit");
		},
		get declined() {
			return t("Didn't edit");
		},
	},
	delete: {
		get doing() {
			return t("Deleting");
		},
		get done() {
			return t("Deleted");
		},
		get waiting() {
			return t("Wants to delete");
		},
		get declined() {
			return t("Didn't delete");
		},
	},
	rename: {
		get doing() {
			return t("Renaming");
		},
		get done() {
			return t("Renamed");
		},
		get waiting() {
			return t("Wants to rename");
		},
		get declined() {
			return t("Didn't rename");
		},
	},
};

/**
 * Text as inline code. The fence is one backtick longer than the longest
 * run inside, and text that starts or ends with one gets a space each side.
 */
export const inlineCode = (text: string): string => {
	const flat = text.replace(/\s+/g, " ").trim();
	const longest = Math.max(0, ...(flat.match(/`+/g) ?? []).map((run) => run.length));
	const fence = "`".repeat(longest + 1);
	const pad = flat.startsWith("`") || flat.endsWith("`") ? " " : "";
	return `${fence}${pad}${flat}${pad}${fence}`;
};

// What a step that didn't finish adds at the end of its line.
const ENDED_EARLY: Partial<Record<ItemStatus, string>> = {
	declined: " (declined)",
	cancelled: " (stopped)",
	interrupted: " (stopped)",
	failed: " (failed)",
};

/** An artifact call's verb and page as one line, such as "Published Weekly report". */
export const artifactWords = (status: ItemStatus, line: ArtifactLine): string =>
	line.target ? `${verbFor(status, line.verbs)} ${line.target}` : verbFor(status, line.verbs);

/**
 * A tool step as lines of Markdown: one for each file a change touched and
 * one for any other step. Empty for anything that isn't a tool step.
 * `names` names the agents and devices an agent tool acted on.
 */
export const toolLines = (item: TimelineItem, names: ToolNames): string[] => {
	const end = ENDED_EARLY[item.status] ?? "";
	const line = (verbs: Verbs, target: string) => `${verbFor(item.status, verbs)} ${target}${end}`;
	switch (item.kind) {
		case "command":
			return [line(TOOL_VERBS.command, inlineCode(item.command.split("\n")[0] ?? ""))];
		case "file_change":
			return item.changes.map((edit) => {
				const path =
					edit.op === "rename" && edit.old_path
						? t("{{value1}} to {{value2}}", {
								value1: inlineCode(edit.old_path),
								value2: inlineCode(edit.path),
							})
						: inlineCode(edit.path);
				const counts =
					edit.op === "modify" ? ` (+${edit.additions} −${edit.deletions})` : "";
				return line(EDIT_VERBS[edit.op], `${path}${counts}`);
			});
		case "file_read":
			return [line(TOOL_VERBS.read, inlineCode(item.path))];
		case "search":
			return [
				line(
					TOOL_VERBS.search,
					item.scope
						? t("{{value1}} in {{value2}}", {
								value1: inlineCode(item.query),
								value2: inlineCode(item.scope),
							})
						: inlineCode(item.query)
				),
			];
		case "web":
			return [
				line(
					WEB_VERBS[item.action],
					item.action === "search" ? inlineCode(item.target) : item.target
				),
			];
		case "tool_call": {
			const artifact = artifactLine(item);
			if (artifact) {
				const note = artifact.note ? `, ${artifact.note}` : "";
				const link =
					artifact.url && artifact.url !== artifact.target ? ` <${artifact.url}>` : "";
				return [`${artifactWords(item.status, artifact)}${note}${link}${end}`];
			}
			const agentTool = agentToolLine(item, names);
			if (agentTool) {
				const note = agentTool.note ? `, ${agentTool.note}` : "";
				return [line(agentTool, `${agentTool.target}${note}`)];
			}
			const name = inlineCode(item.server ? `${item.server}:${item.name}` : item.name);
			return [line(TOOL_VERBS.call, item.title ? `${name}, ${item.title}` : name)];
		}
		default:
			return [];
	}
};
