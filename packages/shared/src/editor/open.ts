// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Follows the behaviour of MIT-licensed code listed in THIRD_PARTY_NOTICES:
// whether a workspace opens in an editor on this Mac, over SSH, or not at all,
// and which editor the Open in button starts with. The rest is MaxHQ's: the
// answer comes from the workspace and its device's link, the same way the app
// decides when it opens one. A container goes over SSH to its alias, once your
// `~/.ssh/config` includes MaxHQ's or you've said you'll add its entry
// yourself. The default editor setting is MaxHQ's too.

import { t } from "../i18n";

import type { DeviceLink, Editors, SshIncludeState, Workspace } from "../bindings";
import type { EditorChoice } from "../data/source";

/** How an editor reaches a workspace, for what the Open in menu shows. */
export type EditorReach =
	| { readonly kind: "local" }
	| { readonly kind: "remote"; readonly host: string }
	/** A WSL distro on this PC, which the editor opens through its WSL support. */
	| { readonly kind: "wsl"; readonly distro: string }
	/** A container whose alias needs the `Include` line, which `line` is. */
	| { readonly kind: "ask"; readonly host: string; readonly line: string }
	/** A container whose `Include` you said no to. `block` is its entry to copy. */
	| { readonly kind: "declined"; readonly host: string; readonly block: string }
	| { readonly kind: "unavailable"; readonly reason: string };

/** What a container's items say while the app checks its SSH setup. */
export const CHECKING_SSH = "Checking the SSH setup";

/**
 * How an editor on this Mac reaches `workspace` through `link`, the link to
 * its device: a folder here opens as it is, and one on another device over
 * that link's SSH host. A container follows `include`, where it stands with
 * your `~/.ssh/config`, which is undefined until the app knows.
 */
export const editorReach = (
	workspace: Workspace,
	link: DeviceLink | undefined,
	include?: SshIncludeState
): EditorReach => {
	if (workspace.location.type === "container") {
		switch (include?.type) {
			case undefined:
				return { kind: "unavailable", reason: CHECKING_SSH };
			case "included":
				return { kind: "remote", host: include.alias };
			case "ask":
				return { kind: "ask", host: include.alias, line: include.line };
			case "declined":
				return { kind: "declined", host: include.alias, block: include.block };
			case "unavailable":
				return { kind: "unavailable", reason: include.reason };
		}
	}
	if (!link) return { kind: "unavailable", reason: t("No route to this device") };
	switch (link.route.type) {
		case "local":
			return { kind: "local" };
		case "ssh":
			return { kind: "remote", host: link.route.host };
		case "wsl":
			return link.route.via === null
				? { kind: "wsl", distro: link.route.distro }
				: {
						kind: "unavailable",
						reason: t(
							"A WSL distro on another PC can't open in an editor from here yet"
						),
					};
	}
};

/**
 * Which editor the Open in button and a file row's button use, set per Mac:
 * the last one used, always the same one, or none, which hides both.
 */
export type DefaultEditor =
	| { readonly kind: "last" }
	| { readonly kind: "editor"; readonly id: string }
	| { readonly kind: "none" };

/** What a Mac starts with. */
export const LAST_USED: DefaultEditor = { kind: "last" };

/** A choice in the default editor setting, and whether this Mac has it. */
export type EditorOption = EditorChoice & { readonly found: boolean };

/**
 * The editors the setting offers: every one the app knows, marked by
 * whether this Mac has it. None until the app says.
 */
export const editorOptions = (editors: Editors | undefined): EditorOption[] =>
	editors?.known.map((e) => ({
		...e,
		found: editors.available.some((a) => a.id === e.id),
	})) ?? [];

/**
 * The editor the buttons use: the default while this Mac has it, or else
 * the last one used while it has that, or else the first. Null with the
 * default None, or when this Mac has no editor.
 */
export const preferredEditor = (
	available: readonly EditorChoice[],
	last: string | null,
	fixed: DefaultEditor = LAST_USED
): EditorChoice | null => {
	if (fixed.kind === "none") return null;
	const find = (id: string | null) => available.find((e) => e.id === id);
	return (
		(fixed.kind === "editor" ? find(fixed.id) : undefined) ?? find(last) ?? available[0] ?? null
	);
};

// Bump when the saved shape changes, so an old setting reads as Last used.
const DEFAULT_EDITOR_VERSION = 1;

/** The default editor as it's saved. */
export const writeDefaultEditor = (choice: DefaultEditor): string =>
	JSON.stringify({ version: DEFAULT_EDITOR_VERSION, ...choice });

/** Reads a saved default editor, or Last used when it doesn't check out. */
export const readDefaultEditor = (raw: string | null): DefaultEditor => {
	if (raw === null) return LAST_USED;
	let saved: unknown;
	try {
		saved = JSON.parse(raw);
	} catch {
		return LAST_USED;
	}
	if (typeof saved !== "object" || saved === null) return LAST_USED;
	const { version, kind, id } = saved as Record<string, unknown>;
	if (version !== DEFAULT_EDITOR_VERSION) return LAST_USED;
	if (kind === "none") return { kind };
	if (kind === "editor" && typeof id === "string" && id !== "") return { kind, id };
	return LAST_USED;
};
