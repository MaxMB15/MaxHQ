// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a call to one of Claude Code's artifact tools reads in the timeline:
// "Published Weekly report" with its link, rather than a bare Artifact row
// and its JSON. maxd fills the item's `artifact` with what the call did,
// and only a link on claude.ai ever opens from it.

import { t } from "../i18n";

import type { TimelineItem } from "../bindings";
import type { Verbs } from "./tool-line";

/** Which of the four tools a call is, for its icon. */
export type ArtifactTool = "page" | "data" | "comments" | "check";

/** What an artifact call's row says. */
export type ArtifactLine = {
	readonly tool: ArtifactTool;
	readonly verbs: Verbs;
	/** The page it acted on, or empty when the verb says it all. */
	readonly target: string;
	/** More about it, such as the file, the version and whose page it is. */
	readonly note: string | null;
	/** The page's link, only when it's on claude.ai. */
	readonly url: string | null;
};

// An https link whose host is claude.ai, with no port or login, and nothing
// after it but the characters a link may hold. The slash right after the
// host shuts out "claude.ai.example.com" and "claude.ai@example.com".
// React Native's URL can't read a host, so this is a plain pattern.
const CLAUDE_LINK = /^https:\/\/claude\.ai\/[A-Za-z0-9\-._~!$&'()*+,;=:@%/?#]*$/;

/**
 * `text` as a link to open, when it's an https link on claude.ai and
 * nothing else, or null. Checked again before every open.
 */
export const claudeLink = (text: string | null | undefined): string | null =>
	text && CLAUDE_LINK.test(text) ? text : null;

const oneLine = (text: string | null | undefined): string =>
	(text ?? "")
		.replace(/\p{Cc}/gu, " ")
		.replace(/\s+/g, " ")
		.trim();

const verbs = (doing: string, done: string, waiting?: string, declined?: string): Verbs => ({
	doing,
	done,
	waiting,
	declined,
});

// The verbs for an action, and whether they stand without a page.
type Words = { readonly verbs: Verbs; readonly alone?: boolean };

const pageWords = (action: string): Words | undefined => {
	switch (action) {
		case "publish":
			return {
				verbs: verbs(
					t("Publishing"),
					t("Published"),
					t("Wants to publish"),
					t("Didn't publish")
				),
			};
		case "read":
			return {
				verbs: verbs(t("Reading"), t("Read"), t("Wants to read"), t("Didn't read")),
			};
		case "list":
			return {
				verbs: verbs(t("Listing your claude.ai pages"), t("Listed your claude.ai pages")),
				alone: true,
			};
		case "open":
			return { verbs: verbs(t("Opening"), t("Opened")) };
		case "delete":
			return {
				verbs: verbs(t("Deleting"), t("Deleted"), t("Wants to delete"), t("Didn't delete")),
			};
		case "pin":
			return { verbs: verbs(t("Pinning"), t("Pinned")) };
		case "unpin":
			return { verbs: verbs(t("Unpinning"), t("Unpinned")) };
		case "quickstart":
			return { verbs: verbs(t("Starting a page"), t("Started a page")), alone: true };
	}
};

const dataWords = (action: string): Words | undefined => {
	switch (action) {
		case "get":
		case "list":
		case "query":
			return { verbs: verbs(t("Reading data from"), t("Read data from")) };
		case "set":
		case "update":
		case "delete":
		case "batch":
			return { verbs: verbs(t("Saving data to"), t("Saved data to")) };
	}
};

const commentWords = (action: string): Words | undefined => {
	switch (action) {
		case "reply":
			return { verbs: verbs(t("Replying to a comment on"), t("Replied to a comment on")) };
		case "list":
		case "status":
			return { verbs: verbs(t("Reading comments on"), t("Read comments on")) };
		case "resolve":
			return { verbs: verbs(t("Resolving a comment on"), t("Resolved a comment on")) };
		case "watch":
			return { verbs: verbs(t("Watching comments on"), t("Watching comments on")) };
		case "unwatch":
			return {
				verbs: verbs(t("Stopped watching comments on"), t("Stopped watching comments on")),
			};
	}
};

const TOOLS: Record<string, { tool: ArtifactTool; words: (action: string) => Words | undefined }> =
	{
		Artifact: { tool: "page", words: pageWords },
		ArtifactData: { tool: "data", words: dataWords },
		ArtifactComments: { tool: "comments", words: commentWords },
		ArtifactCheck: {
			tool: "check",
			words: () => ({ verbs: verbs(t("Checking"), t("Checked")) }),
		},
	};

/**
 * The row for a call to an artifact tool, or undefined for any other item.
 * Its target is the page's title, else its link, else what the call named.
 * An action this version doesn't know reads as "Used Artifacts on".
 */
export const artifactLine = (item: TimelineItem): ArtifactLine | undefined => {
	if (item.kind !== "tool_call" || !item.artifact) return undefined;
	const call = item.artifact;
	const known = TOOLS[item.name];
	const words = known?.words(call.action) ?? {
		verbs: verbs(t("Using Artifacts on"), t("Used Artifacts on")),
	};
	const title = oneLine(call.title);
	const url = claudeLink(call.url);
	const detail = oneLine(call.detail);
	const target = words.alone ? "" : title || url || detail || t("a page");
	const version = oneLine(call.version);
	const note = [
		detail === target ? "" : detail,
		version && (/^v\d/i.test(version) ? version : `v${version}`),
		call.owned === false ? t("someone else's") : "",
	].filter(Boolean);
	return {
		tool: known?.tool ?? "page",
		verbs: words.verbs,
		target,
		note: note.length > 0 ? note.join(", ") : null,
		url,
	};
};
