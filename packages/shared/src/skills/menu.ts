// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the skill menu's
// lookup and search. The `$` menu lists the skills an agent's CLI offers that you
// can call, once per name, the first a scan found winning. A query ranks
// the name first, then the label, then the descriptions.

import { t } from "../i18n";

import type { EffectiveSkill, Vendor } from "../bindings";

/** Each skill once by name, ignoring case, the first one found winning. */
export const dedupeSkillsByName = (skills: readonly EffectiveSkill[]): EffectiveSkill[] => {
	const seen = new Set<string>();
	return skills.filter((skill) => {
		const name = skill.name.trim().toLowerCase();
		if (seen.has(name)) return false;
		seen.add(name);
		return true;
	});
};

/**
 * Whether you can call a skill. Disabled skills and agent-only ones
 * (Claude's `user-invocable: false`) stay out.
 */
export const isUserInvocable = (skill: Pick<EffectiveSkill, "enabled" | "user_invocable">) =>
	skill.enabled && skill.user_invocable !== false;

const titleCase = (name: string): string =>
	name
		.split(/[-_\s]+/u)
		.filter(Boolean)
		.map((word) => word[0]!.toUpperCase() + word.slice(1))
		.join(" ");

/** The name a menu shows: the skill's own display name, or its name in title case. */
export const skillLabel = (skill: Pick<EffectiveSkill, "name" | "display_name">): string =>
	skill.display_name?.trim() || titleCase(skill.name);

/** What a menu row says under the label. */
export const skillHint = (skill: EffectiveSkill): string =>
	skill.short_description ??
	skill.description ??
	(skill.scope ? t("{{value1}} skill", { value1: skill.scope }) : t("Skill"));

/** How well `value` matches `query` from `base` up, lower first, or null for no match. */
const scoreText = (value: string, query: string, base: number): number | null => {
	if (!value) return null;
	if (value === query) return base;
	if (value.startsWith(query)) return base + 2;
	if (value.split(/[-_/\s]/u).some((word) => word.startsWith(query))) return base + 4;
	if (value.includes(query)) return base + 6;
	return null;
};

const scoreSkill = (skill: EffectiveSkill, query: string): number | null => {
	const scores = [
		scoreText(skill.name.toLowerCase(), query, 0),
		scoreText(skillLabel(skill).toLowerCase(), query, 1),
		scoreText(skill.short_description?.toLowerCase() ?? "", query, 20),
		scoreText(skill.description?.toLowerCase() ?? "", query, 30),
		scoreText(skill.scope?.toLowerCase() ?? "", query, 40),
	].filter((score): score is number => score !== null);
	return scores.length === 0 ? null : Math.min(...scores);
};

/**
 * The `$` menu for an agent of `cli`: what a scan found that it offers and
 * you can call, best match first. `query` is what follows the `$`.
 */
export const skillMenu = (
	skills: readonly EffectiveSkill[],
	cli: Vendor,
	query: string
): EffectiveSkill[] => {
	const offered = dedupeSkillsByName(
		skills.filter((skill) => skill.cli === cli && isUserInvocable(skill))
	);
	const wanted = query
		.replace(/^\p{Sc}+/u, "")
		.trim()
		.toLowerCase();
	if (!wanted) return offered;
	return offered
		.map((skill) => ({ skill, score: scoreSkill(skill, wanted) }))
		.filter((entry): entry is { skill: EffectiveSkill; score: number } => entry.score !== null)
		.sort(
			(a, b) =>
				a.score - b.score ||
				skillLabel(a.skill)
					.toLowerCase()
					.localeCompare(skillLabel(b.skill).toLowerCase()) ||
				a.skill.name.localeCompare(b.skill.name)
		)
		.map((entry) => entry.skill);
};
