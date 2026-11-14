// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The skills each CLI sees in a place, as a scan found them, turned into
// the rows the Skills tab lists, and what they take of each CLI's listing.
// The marks come from the scan, which alone knows each CLI's precedence;
// nothing here works them out.

import { t } from "../i18n";

import type { EffectiveSkill, SkillMark, SkillPlace, Vendor } from "../bindings";

/** The places in the order the rows list them. */
const PLACES: readonly SkillPlace[] = ["project", "always", "global", "plugin", "system"];

/** The CLIs in the order the views list them. */
const CLIS: readonly Vendor[] = ["claude", "codex", "cursor", "acp"];

/** What a CLI is called on the Skills tab. */
export const cliLabel = (cli: Vendor): string =>
	({ claude: "Claude", codex: "Codex", cursor: "Cursor", acp: t("ACP agents") })[cli];

/**
 * Where a skill comes from, in words. A claude.ai skill sits in Claude's
 * global folder under `synced`, and the scan gives it that scope.
 */
export const originLabel = (skill: EffectiveSkill): string => {
	if (skill.mark === "vendored") return t("Vendored");
	if (skill.scope === "synced") return t("Synced");
	return {
		project: t("Project"),
		global: t("Global"),
		always: t("Every agent"),
		plugin: t("Plugin"),
		system: t("System"),
	}[skill.place];
};

/** What a mark says beside a skill. */
export const markLabel = (mark: SkillMark): string =>
	({
		shadowed: t("A global skill of the same name runs instead"),
		same: t("Also in another folder, the same copy"),
		conflict: t("Also in another folder, a different copy"),
		vendored: t("Another installer owns it"),
	})[mark];

/** One CLI's skills, in the order the tab lists them. */
export type CliSkills = { readonly cli: Vendor; readonly skills: readonly EffectiveSkill[] };

/**
 * A scan's skills grouped by CLI, each group by place and then by name.
 * CLIs that see nothing are left out.
 */
export const skillsByCli = (skills: readonly EffectiveSkill[]): CliSkills[] =>
	CLIS.map((cli) => ({
		cli,
		skills: skills
			.filter((skill) => skill.cli === cli)
			.sort(
				(a, b) =>
					PLACES.indexOf(a.place) - PLACES.indexOf(b.place) ||
					a.name.localeCompare(b.name) ||
					a.path.localeCompare(b.path)
			),
	})).filter((group) => group.skills.length > 0);

/** The most characters of a description Claude lists. */
export const MAX_ENTRY_DESCRIPTION = 1536;

/** A listing's size when the model's context isn't known, in characters. */
export const FALLBACK_LISTING = 8_000;

/** About how many characters a token holds. */
const CHARS_PER_TOKEN = 4;

/** The characters around each listed entry: `- `, `: ` and a line break. */
const ENTRY_OVERHEAD = 5;

/** How many characters `text` has, counted as the core counts them. */
const chars = (text: string): number => [...text].length;

/** About how many characters one skill adds to a listing. */
const entryChars = (skill: EffectiveSkill): number =>
	chars(skill.name) +
	Math.min(chars(skill.description ?? ""), MAX_ENTRY_DESCRIPTION) +
	ENTRY_OVERHEAD;

/**
 * The listing's size for `cli` in characters: 1% of the context for
 * Claude and 2% for Codex, or 8,000 when the context isn't known. Cursor
 * and ACP agents don't say, so they have none.
 */
export const listingLimit = (cli: Vendor, windowTokens: number | null): number | null => {
	const percent = cli === "claude" ? 1 : cli === "codex" ? 2 : null;
	if (percent === null) return null;
	if (windowTokens === null) return FALLBACK_LISTING;
	return Math.floor((windowTokens * CHARS_PER_TOKEN * percent) / 100);
};

/** One CLI's listing against its size. */
export type Budget = {
	readonly cli: Vendor;
	/** The skills it lists. */
	readonly listed: number;
	/** About how many characters they take. */
	readonly used: number;
	/** The listing's size, when the CLI has one. */
	readonly limit: number | null;
	/** Whether it's past its size, so the CLI drops descriptions. */
	readonly over: boolean;
};

/**
 * What the skills `cli` sees take of its listing, with a model whose
 * window holds `windowTokens`. Only skills it offers and the model may
 * call itself are listed.
 */
export const budgetOf = (
	cli: Vendor,
	skills: readonly EffectiveSkill[],
	windowTokens: number | null
): Budget => {
	const listed = skills.filter(
		(skill) => skill.cli === cli && skill.enabled && skill.user_invocation_only !== true
	);
	const used = listed.reduce((sum, skill) => sum + entryChars(skill), 0);
	const limit = listingLimit(cli, windowTokens);
	return { cli, listed: listed.length, used, limit, over: limit !== null && used > limit };
};

/** Whether going from `before` to `after` pushes descriptions out, to warn before an apply. */
export const pushesOut = (before: Budget, after: Budget): boolean => !before.over && after.over;
