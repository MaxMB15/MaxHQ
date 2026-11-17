// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Shell mode in the composer: the `!` that starts it, the Backspace that
// leaves it, where a command runs, and the command a code block holds.
// Plain functions, so the desktop and the phone follow the same rules.

import type { AgentId, DeviceId } from "../bindings";
import type { ViewState } from "../view/replica";
import { deviceName } from "../view/select";

/** What the composer sends: a message, a command, or a command run privately with `!!`. */
export type ShellMode = "message" | "shell" | "private";

/**
 * The mode a `!` typed at the very start of the box goes to: a message
 * becomes a command and a command a private one. Null when it's a plain `!`.
 */
export const bangStep = (mode: ShellMode): ShellMode | null =>
	mode === "message" ? "shell" : mode === "shell" ? "private" : null;

/** The mode Backspace at the very start goes back to, one `!` at a time. */
export const backStep = (mode: ShellMode): ShellMode => (mode === "private" ? "shell" : "message");

/** The languages whose code blocks hold commands to run. */
export const SHELL_LANGUAGES: ReadonlySet<string> = new Set([
	"sh",
	"bash",
	"zsh",
	"shell",
	"console",
	"fish",
]);

/** Whether a code block in this language holds commands to run. */
export const isShellLanguage = (lang: string | null | undefined): boolean =>
	lang != null && SHELL_LANGUAGES.has(lang.trim().toLowerCase());

const PROMPT = /^\s*\$ /;

/**
 * The commands of a console block: each line after a `$ ` prompt, and the
 * lines it goes on to after a trailing `\`. The output between them stays
 * out. A block with no prompt is taken whole.
 */
export const consoleCommand = (code: string): string => {
	const lines = code.split("\n");
	if (!lines.some((line) => PROMPT.test(line))) return code;
	const kept: string[] = [];
	let goesOn = false;
	for (const line of lines) {
		if (PROMPT.test(line)) kept.push(line.replace(PROMPT, ""));
		else if (goesOn) kept.push(line);
		else continue;
		goesOn = line.trimEnd().endsWith("\\");
	}
	return kept.join("\n");
};

/**
 * The command a shell code block holds, ready for the composer: the code
 * as it is, without its last line breaks, and for a console block only its
 * commands.
 */
export const commandOfBlock = (code: string, lang: string | null | undefined): string => {
	const text = code.replace(/\n+$/, "");
	return lang?.trim().toLowerCase() === "console" ? consoleCommand(text) : text;
};

/** A folder with the home folder written as `~`, the way a prompt shows it. */
export const homeShort = (path: string): string => {
	const home = /^(\/Users\/[^/]+|\/home\/[^/]+|\/root)(?=\/|$)/.exec(path);
	return home ? `~${path.slice(home[0].length)}` : path;
};

/** Where a command typed in an agent's composer runs. */
export type RunPlace = {
	readonly device: DeviceId;
	/** The device's name, such as "Mac". */
	readonly deviceName: string;
	/** The agent's folder, with the home folder as `~`. */
	readonly folder: string;
	/** The container it runs in, for an agent in one. */
	readonly container: string | null;
};

/** Where the agent's commands run: its folder, in its container when it has one, on its device. */
export const runPlace = (view: ViewState, agent: AgentId): RunPlace | null => {
	const record = view.tables.agent.get(agent);
	const workspace = record ? view.tables.workspace.get(record.workspace) : undefined;
	if (!workspace) return null;
	const { location } = workspace;
	return {
		device: workspace.device,
		deviceName: deviceName(view, workspace.device),
		folder: homeShort(location.path),
		container: location.type === "container" ? location.instance : null,
	};
};

/** The program a command starts, such as `sudo` for `sudo systemctl restart nginx`. */
export const programOf = (command: string): string =>
	command.trim().split(/\s+/)[0]?.split("/").at(-1) ?? "";
