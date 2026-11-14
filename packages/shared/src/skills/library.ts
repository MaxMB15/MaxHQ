// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the views read about skills from the view: whether a device takes
// skills requests, the library and its sources, each project's lock and
// checkouts, and the proposals waiting for you. The library lives on one
// device, the holder, and each project's lock on the device that owns it.

import { t } from "../i18n";

import type {
	AgentId,
	CheckoutSkills,
	DeviceId,
	LockedSkill,
	ProjectId,
	ProjectSkills,
	SkillInfo,
	SkillLibrary,
	SkillLock,
	SkillProposal,
	SkillSource,
	SourceKind,
} from "../bindings";
import { all, type ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

/** Whether `device`'s `maxd` takes skills requests. */
export const offersSkills = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.skills === true;

/**
 * The skill library, from the device that holds it. While more than one
 * device publishes one, this Mac's own wins, since any device can say it holds
 * one, and then the connected holder's, as during a move.
 */
export const skillLibrary = (view: ViewState): SkillLibrary | undefined => {
	const libraries = all(view, "skill_library").sort((a, b) => a.device.localeCompare(b.device));
	const here = (library: SkillLibrary) =>
		all(view, "device_link").some(
			(link) => link.device === library.device && link.route.type === "local"
		);
	const connected = (library: SkillLibrary) =>
		linkForDevice(view, library.device)?.state.state === "connected";
	return libraries.find(here) ?? libraries.find(connected) ?? libraries[0];
};

/** Where a source's skills come from, in words: its URL or its folder. */
export const sourceWhere = (kind: SourceKind): string =>
	kind.kind === "git" ? kind.url : kind.path;

/** What a skill does beyond its instructions, and whether its source is someone else's. */
export type SkillBadge = "scripts" | "hooks" | "allowed_tools" | "third_party";

/**
 * A skill's badges, in the order the views show them. A proposal has
 * flags too, and a source only when it changes a skill the library has.
 */
export const skillBadges = (
	skill: Pick<SkillInfo, "flags">,
	source: Pick<SkillSource, "trust"> | undefined
): SkillBadge[] => [
	...(skill.flags.scripts ? (["scripts"] as const) : []),
	...(skill.flags.hooks ? (["hooks"] as const) : []),
	...(skill.flags.allowed_tools ? (["allowed_tools"] as const) : []),
	...(source?.trust === "third_party" ? (["third_party"] as const) : []),
];

/** What a badge says. */
export const badgeLabel = (badge: SkillBadge): string =>
	({
		scripts: t("Scripts"),
		hooks: t("Hooks"),
		allowed_tools: t("Allowed tools"),
		third_party: t("Third party"),
	})[badge];

/** One skill of the library, with the source it's in. */
export type LibrarySkill = { readonly skill: SkillInfo; readonly source: SkillSource };

/**
 * The library's skills whose name or description has every word of
 * `query`, by name. An empty query gives them all.
 */
export const searchLibrary = (library: SkillLibrary, query: string): LibrarySkill[] => {
	const words = query.toLowerCase().split(/\s+/u).filter(Boolean);
	return library.sources
		.flatMap((source) => source.skills.map((skill) => ({ skill, source })))
		.filter(({ skill }) => {
			const text = `${skill.name} ${skill.description}`.toLowerCase();
			return words.every((word) => text.includes(word));
		})
		.sort((a, b) => a.skill.name.localeCompare(b.skill.name));
};

/**
 * A library skill as a lock takes it, at its source's commit. A third
 * party's skill is pinned, so it stays put when its source moves on.
 */
export const lockedFrom = ({ skill, source }: LibrarySkill): LockedSkill => ({
	name: skill.name,
	source: source.id,
	path: skill.path,
	commit: source.commit,
	tree: skill.tree,
	pinned: source.trust === "third_party",
});

/** Why a project can't take a skill of this name, or null when it can. */
export const cantLock = (library: SkillLibrary, lock: SkillLock, name: string): string | null => {
	if (library.always.includes(name))
		return t("Every agent already gets {{name}}", { name: name });
	if (lock.skills.some((skill) => skill.name === name))
		return t("The project already has {{name}}", { name: name });
	return null;
};

/**
 * `into` with the skills of `from` it lacks, for "Add from project".
 * Names it already has or the always set holds stay out.
 */
export const addFromProject = (
	library: SkillLibrary,
	into: SkillLock,
	from: SkillLock
): SkillLock => {
	const skills = [...into.skills];
	for (const skill of from.skills) {
		if (cantLock(library, { ...into, skills }, skill.name) === null) skills.push(skill);
	}
	return { ...into, skills };
};

/**
 * A project's skills on each device that has them, by device. Each device
 * publishes its own checkouts.
 */
export const projectSkillsOf = (view: ViewState, project: ProjectId): ProjectSkills[] =>
	all(view, "project_skills")
		.filter((skills) => skills.project === project)
		.sort((a, b) => a.device.localeCompare(b.device));

/** A checkout whose copies changed since they landed, on the device it's on. */
export type Drift = { readonly device: DeviceId; readonly checkout: CheckoutSkills };

/** A project's checkouts with drift, for the drift banner. */
export const driftOf = (view: ViewState, project: ProjectId): Drift[] =>
	projectSkillsOf(view, project).flatMap((skills) =>
		skills.checkouts
			.filter((checkout) => checkout.drift.length > 0)
			.map((checkout) => ({ device: skills.device, checkout }))
	);

/** A project's checkouts with a link that stopped skills landing, for the banner. */
export const outsideOf = (view: ViewState, project: ProjectId): Drift[] =>
	projectSkillsOf(view, project).flatMap((skills) =>
		skills.checkouts
			.filter((checkout) => (checkout.outside ?? []).length > 0)
			.map((checkout) => ({ device: skills.device, checkout }))
	);

/**
 * Whether an agent waits for a restart to see its project's skills. Only
 * Cursor and ACP agents read skills at start, so only they wait.
 */
export const waitsForSkills = (view: ViewState, agent: AgentId): boolean => {
	const found = view.tables.agent.get(agent);
	if (!found || (found.vendor !== "cursor" && found.vendor !== "acp")) return false;
	const workspace = view.tables.workspace.get(found.workspace);
	if (!workspace?.project) return false;
	return projectSkillsOf(view, workspace.project).some(
		(skills) =>
			skills.device === workspace.device &&
			skills.checkouts.some(
				(checkout) => checkout.needs_restart && checkout.path === workspace.location.path
			)
	);
};

/** The proposals waiting for you, newest first, only `project`'s when it's given. */
export const skillProposals = (view: ViewState, project?: ProjectId): SkillProposal[] =>
	all(view, "skill_proposal")
		.filter((proposal) => project === undefined || proposal.project === project)
		.sort((a, b) => b.created_at - a.created_at || a.id.localeCompare(b.id));
