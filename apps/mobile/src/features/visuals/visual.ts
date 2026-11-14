// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the phone needs to show a visual whole: which one, from a feed's item
// or a fenced block, the theme it draws in, which of the page's links may
// open, and what its bars say. The screen draws from this.

import { t } from "@maxhq/shared/i18n";

import { createStore } from "zustand/vanilla";

import type { AgentId, AttachmentRef } from "@maxhq/shared/bindings";
import { listInWords } from "@maxhq/shared/status/format";
import { type Blocked, blockedLoad, THEME_VARIABLES } from "@maxhq/shared/visuals/frame";
import { fromVisualFence } from "@maxhq/shared/visuals/fence";
import { opensFromVisual } from "@maxhq/shared/visuals/inline";

import { color, radius } from "../../theme";

/** A visual to show whole: a page in hand, or the copy on an agent's device. */
export type VisualToOpen = { readonly title: string } & (
	| { readonly kind: "page"; readonly html: string }
	| { readonly kind: "device"; readonly agent: AgentId; readonly path: string }
);

/** The visual an agent showed, from its item's attachment. */
export const visualOfItem = (
	agent: AgentId,
	item: { readonly title: string; readonly attachment: AttachmentRef }
): VisualToOpen => ({
	kind: "device",
	title: item.title.trim(),
	agent,
	path: item.attachment.path,
});

/** The visual a block of Markdown is, when it's an `html visual` fence. */
export const visualOfBlock = (source: string): VisualToOpen | null => {
	const visual = fromVisualFence(source);
	return visual ? { kind: "page", title: visual.title.trim(), html: visual.html } : null;
};

/** What a visual's row says it is. */
export const visualName = (title: string): string => title.trim() || t("Visual");

// The desktop's dark chart colors, which the phone's theme doesn't name.
const CHARTS = ["#4692f6", "#199e70", "#c98500", "#d95926", "#9085e9", "#2aa5c0"];

/** The theme's variables a visual's page draws with, in the phone's colors. */
export const VISUAL_VARIABLES: Readonly<Record<(typeof THEME_VARIABLES)[number], string>> = {
	"--background": color.background,
	"--foreground": color.foreground,
	"--muted": color.secondary,
	"--muted-foreground": color.mutedForeground,
	"--border": color.border,
	"--accent": color.accent,
	"--primary": color.primary,
	"--chart-1": CHARTS[0]!,
	"--chart-2": CHARTS[1]!,
	"--chart-3": CHARTS[2]!,
	"--chart-4": CHARTS[3]!,
	"--chart-5": CHARTS[4]!,
	"--chart-6": CHARTS[5]!,
	"--font-sans": "-apple-system, system-ui, sans-serif",
	"--font-mono": "Menlo, monospace",
	"--radius": `${radius.md}px`,
};

// A page can say it was clicked when it wasn't, so the screen opens one link
// a second at most, as the desktop does.
const LINK_GAP_MS = 1000;

/**
 * Says whether a link a page reports may open in the browser now: a web or
 * mail address, and not within a second of the last one opened.
 */
export const linkGate = () => {
	let last = -Infinity;
	return (url: string, now: number): boolean => {
		if (!opensFromVisual(url) || now - last < LINK_GAP_MS) return false;
		last = now;
		return true;
	};
};

/** The bar for the libraries a page asked for that the phone doesn't carry. */
export const missingText = (missing: readonly string[]): string | null =>
	missing.length === 0
		? null
		: t("Not found: {{names}}. A visual has only its own page and MaxHQ's libraries.", {
				names: listInWords(missing),
			});

/** A script's error in a visual's page. */
export type PageError = { readonly message: string; readonly line: number | null };

/** The bar for a page's errors: the first, and how many more came. */
export const errorText = (first: PageError, count: number): string => {
	const error =
		first.line === null
			? first.message
			: t("{{message}} (line {{line}})", { message: first.message, line: first.line });
	return count > 1
		? t("{{error}}, and {{count}} more errors", { error, count: count - 1 })
		: error;
};

/** The hosts the page tried to load from, each once, from the policy's reports. */
export const blockedHosts = (
	hosts: readonly string[],
	report: { readonly url: string; readonly what: string }
): readonly string[] => {
	const blocked: Blocked | null = blockedLoad(report.url, report.what);
	return !blocked || hosts.includes(blocked.host) ? hosts : [...hosts, blocked.host];
};

/** The bar for what the content policy kept the page from loading. */
export const blockedText = (hosts: readonly string[]): string | null =>
	hosts.length === 0
		? null
		: t("Blocked loading from {{hosts}}. Visuals don't reach the network.", {
				hosts: listInWords(hosts),
			});

/** The visual shown whole, if one is. */
export const shownVisual = createStore<{ visual: VisualToOpen | null }>(() => ({ visual: null }));

/** Shows a visual whole, over the whole app. */
export const openVisual = (visual: VisualToOpen): void => shownVisual.setState({ visual });

/** Closes the visual shown whole. */
export const closeVisual = (): void => shownVisual.setState({ visual: null });
