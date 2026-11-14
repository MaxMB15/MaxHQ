// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the cut in the middle
// of a long name. The rest is
// MaxHQ's: what each pill says, and what a screen reader hears.

import { t } from "../i18n";

import type { AttachmentKind } from "../bindings";

import type { GitItemRef } from "./draft";
import type { GitItemState } from "./wire";

/** The longest label a pill shows before it cuts the middle out. */
export const PILL_LABEL_MAX = 36;

/** A long name with its middle cut out, keeping the end, where the extension is. */
export const middleTruncate = (name: string, maxCharacters = PILL_LABEL_MAX): string => {
	const characters = Array.from(name);
	if (characters.length <= maxCharacters) return name;
	if (maxCharacters <= 0) return "";
	if (maxCharacters === 1) return "…";
	const available = maxCharacters - 1;
	const suffixLength = Math.min(available - 1, available >= 18 ? 14 : Math.ceil(available / 2));
	const prefixLength = available - suffixLength;
	const suffix = suffixLength === 0 ? "" : characters.slice(-suffixLength).join("");
	return `${characters.slice(0, prefixLength).join("")}…${suffix}`;
};

/** Whether a name is one MaxHQ gave a pasted picture. */
export const isPastedImage = (name: string): boolean => /^pasted-image(-\d+)?\.png$/.test(name);

/** What a file's pill says: "Image 1" for a pasted picture, else its name, a folder's with a slash. */
export const fileLabel = (
	file: { readonly name: string; readonly kind: AttachmentKind | "missing" },
	number: number | undefined
): string => {
	if (file.kind === "image" && isPastedImage(file.name))
		return t("Image {{number____1}}", { number____1: number ?? 1 });
	if (file.kind === "folder" && !file.name.endsWith("/")) return `${file.name}/`;
	return file.name;
};

/** How many characters of a quote its pill shows. */
const QUOTE_WORDS = 28;

/** A quote's first words in quote marks. */
export const quoteLabel = (text: string): string => {
	const flat = text.replace(/\s+/g, " ").trim();
	if (flat.length <= QUOTE_WORDS) return `“${flat}”`;
	const cut = flat.slice(0, QUOTE_WORDS);
	const space = cut.lastIndexOf(" ");
	return `“${(space > QUOTE_WORDS / 2 ? cut.slice(0, space) : cut).trimEnd()}…”`;
};

/** A note's title to show, or "Untitled" when it has none. */
export const noteLabel = (title: string): string => (title.trim() === "" ? t("Untitled") : title);

/** A visual's label, its title, or "Visual" for one with none. */
export const visualLabel = (title: string): string => (title.trim() === "" ? t("Visual") : title);

/** A pull request's or issue's label, such as "#123 Fix the cache". */
export const gitItemLabel = (item: Pick<GitItemRef, "number" | "title">): string =>
	`#${item.number} ${item.title}`;

const STATE_WORDS: Record<GitItemState, string> = {
	open: "open",
	draft: "draft",
	merged: "merged",
	closed: "closed",
	completed: "completed",
	get not_planned() {
		return t("not planned");
	},
};

/** A pull request's or issue's state in words, such as "not planned". */
export const gitStateWords = (state: GitItemState): string => STATE_WORDS[state];

/** "Pull request" or "Issue". */
export const gitKindWords = (item: Pick<GitItemRef, "kind">): string =>
	item.kind === "pull_request" ? t("Pull request") : t("Issue");

/** Where a file's pill is on its way to the agent. */
export type FileProgress =
	| { readonly state: "ready" }
	| { readonly state: "staging" }
	| { readonly state: "uploading"; readonly percent: number | null }
	| { readonly state: "failed"; readonly error: string };

const KIND_WORDS: Record<AttachmentKind | "missing", string> = {
	get image() {
		return t("Image");
	},
	pdf: "PDF",
	get file() {
		return t("File");
	},
	get folder() {
		return t("Folder");
	},
	get missing() {
		return t("Missing file");
	},
};

/**
 * What a screen reader hears for a file's pill, such as "Image 1,
 * shot.png, 240 KB, uploading, 40 percent".
 */
export const fileAria = (
	file: { readonly name: string; readonly kind: AttachmentKind | "missing" },
	number: number | undefined,
	size: string | null,
	progress: FileProgress
): string => {
	const kind = KIND_WORDS[file.kind];
	const parts = [
		file.kind === "image" || file.kind === "pdf" ? `${kind} ${number ?? 1}` : kind,
		file.name,
	];
	if (size !== null) parts.push(size);
	switch (progress.state) {
		case "staging":
			parts.push(t("getting ready"));
			break;
		case "uploading":
			parts.push("uploading");
			if (progress.percent !== null)
				parts.push(t("{{value1}} percent", { value1: progress.percent }));
			break;
		case "failed":
			parts.push(t("failed, {{value1}}", { value1: progress.error }));
			break;
		case "ready":
			break;
	}
	return parts.join(", ");
};
