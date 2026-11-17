// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the project's Skills tab works out before it sends a lock: which
// devices keep the project's skills, which place a scan looks at, a lock
// with a skill taken out, pinned or moved to its source's newest copy, and
// what an apply would do to each CLI's listing.

import { t } from "../i18n";

import type {
	AgentPlace,
	DeviceId,
	EffectiveSkill,
	GlobalsPlan,
	LockedSkill,
	ProjectId,
	ProjectSkills,
	ProposalAction,
	ProposalDiff,
	ProposalSeen,
	SkillInfo,
	SkillLibrary,
	SkillLock,
	SkillProposal,
	SkillSource,
	TreeHash,
	Vendor,
} from "../bindings";
import { all, type ViewState } from "../view/replica";

import { budgetOf, type Budget, pushesOut, skillsByCli } from "./effective";
import { offersSkills, projectSkillsOf } from "./library";

/** A lock with nothing in it, for a device that keeps none yet. */
export const EMPTY_LOCK: SkillLock = { skills: [], committed: false };

/**
 * The devices that keep the project's skills or could: each that
 * publishes them, and each that holds a checkout of it and takes skills
 * requests. By id, so the order holds still.
 */
export const skillDevices = (view: ViewState, project: ProjectId): DeviceId[] => {
	const devices = new Set(projectSkillsOf(view, project).map((skills) => skills.device));
	for (const workspace of all(view, "workspace")) {
		if (workspace.project === project && offersSkills(view, workspace.device)) {
			devices.add(workspace.device);
		}
	}
	return [...devices].sort();
};

/** The project's skills on `device`, if it keeps any. */
export const projectSkillsOn = (
	view: ViewState,
	project: ProjectId,
	device: DeviceId
): ProjectSkills | undefined =>
	projectSkillsOf(view, project).find((skills) => skills.device === device);

/**
 * Where a scan for the project on `device` looks: its first workspace
 * there, or else the first checkout the device reports, or null for none.
 */
export const scanPlaceOf = (
	view: ViewState,
	project: ProjectId,
	device: DeviceId
): AgentPlace | null => {
	const workspace = all(view, "workspace")
		.filter((w) => w.project === project && w.device === device)
		.sort((a, b) => a.id.localeCompare(b.id))[0];
	if (workspace) return { type: "workspace", workspace: workspace.id };
	const checkout = projectSkillsOn(view, project, device)?.checkouts[0];
	return checkout ? { type: "folder", path: checkout.path } : null;
};

/** Whether a checkout still lacks the lock's skills, so Apply has work to do. */
export const needsApply = (skills: ProjectSkills | undefined): boolean =>
	skills?.checkouts.some((checkout) => !checkout.applied) ?? false;

/** `lock` without the skill called `name`. */
export const withoutSkill = (lock: SkillLock, name: string): SkillLock => ({
	...lock,
	skills: lock.skills.filter((skill) => skill.name !== name),
});

/** `lock` with the skill called `name` pinned, or unpinned. */
export const pinSkill = (lock: SkillLock, name: string, pinned: boolean): SkillLock => ({
	...lock,
	skills: lock.skills.map((skill) => (skill.name === name ? { ...skill, pinned } : skill)),
});

/** The library's copy of a locked skill, if its source still has it. */
export const libraryCopy = (library: SkillLibrary, locked: LockedSkill): SkillInfo | undefined =>
	library.sources
		.find((source) => source.id === locked.source)
		?.skills.find((skill) => skill.name === locked.name);

/**
 * Where a locked skill comes from, in words: its source's name and the
 * commit it was taken at, or that the source no longer has it.
 */
export const lockedWhere = (library: SkillLibrary, locked: LockedSkill): string => {
	if (!libraryCopy(library, locked)) return t("Its source no longer has it");
	const from = library.sources.find((s) => s.id === locked.source);
	return `${from?.name ?? locked.source}${locked.commit ? t(" at {{commit}}", { commit: locked.commit }) : ""}`;
};

/**
 * A locked skill as its source has it now, or null when it's already
 * there, gone from the source or pinned.
 */
export const updateOf = (library: SkillLibrary, locked: LockedSkill): LockedSkill | null => {
	if (locked.pinned) return null;
	const source = library.sources.find((s) => s.id === locked.source);
	const skill = libraryCopy(library, locked);
	if (!source || !skill || skill.tree === locked.tree) return null;
	return { ...locked, path: skill.path, commit: source.commit, tree: skill.tree };
};

/** `lock` with each unpinned skill moved to its source's newest copy. */
export const updatedLock = (library: SkillLibrary, lock: SkillLock): SkillLock => ({
	...lock,
	skills: lock.skills.map((skill) => updateOf(library, skill) ?? skill),
});

/** Whether any skill of `lock` has a newer copy to move to. */
export const hasUpdates = (library: SkillLibrary, lock: SkillLock): boolean =>
	lock.skills.some((skill) => updateOf(library, skill) !== null);

/**
 * The biggest context window among the project's agents of `cli` that
 * report one, or null when none does.
 */
export const windowOf = (view: ViewState, project: ProjectId, cli: Vendor): number | null => {
	let window: number | null = null;
	for (const agent of all(view, "agent")) {
		if (agent.vendor !== cli || !agent.context) continue;
		if (view.tables.workspace.get(agent.workspace)?.project !== project) continue;
		window = Math.max(window ?? 0, agent.context.window_tokens);
	}
	return window;
};

/** Each CLI's listing in a scan, sized by the project's agents' windows. */
export const budgetsOf = (
	view: ViewState,
	project: ProjectId,
	skills: readonly EffectiveSkill[]
): Budget[] =>
	skillsByCli(skills).map(({ cli }) => budgetOf(cli, skills, windowOf(view, project, cli)));

/**
 * What a scan would find once `lock` lands: each CLI's project skills
 * MaxHQ put there swapped for the lock's, as the library describes them.
 * Skills another installer owns stay.
 */
export const afterApply = (
	library: SkillLibrary,
	lock: SkillLock,
	skills: readonly EffectiveSkill[]
): EffectiveSkill[] => {
	const kept = skills.filter((skill) => skill.place !== "project" || skill.mark === "vendored");
	const clis = [...new Set(skills.map((skill) => skill.cli))];
	const landed = clis.flatMap((cli) =>
		lock.skills.map((locked): EffectiveSkill => ({
			name: locked.name,
			description: libraryCopy(library, locked)?.description ?? null,
			path: locked.path,
			scope: null,
			enabled: true,
			display_name: null,
			short_description: null,
			user_invocation_only: null,
			user_invocable: null,
			place: "project",
			cli,
			mark: null,
		}))
	);
	return [...kept, ...landed];
};

/** The CLIs an apply of `lock` would push past their listing, so the tab warns first. */
export const pushedOut = (
	view: ViewState,
	project: ProjectId,
	library: SkillLibrary,
	lock: SkillLock,
	skills: readonly EffectiveSkill[]
): Vendor[] => {
	const after = afterApply(library, lock, skills);
	return skillsByCli(after)
		.map(({ cli }) => cli)
		.filter((cli) => {
			const window = windowOf(view, project, cli);
			return pushesOut(budgetOf(cli, skills, window), budgetOf(cli, after, window));
		});
};

/** Who a proposal is from, in words: its agent's title, or "an agent". */
export const proposalFrom = (view: ViewState, proposal: SkillProposal): string =>
	(proposal.agent ? view.tables.agent.get(proposal.agent)?.title : undefined) ?? t("an agent");

/**
 * A proposal's title in the inbox and its viewer: "Skill change from" its
 * agent, or "Skill change in" its project when no agent is known.
 */
export const proposalTitle = (view: ViewState, proposal: SkillProposal): string => {
	const agent = proposal.agent ? view.tables.agent.get(proposal.agent) : undefined;
	if (agent) return t("Skill change from {{agent_title}}", { agent_title: agent.title });
	const project = view.tables.project.get(proposal.project);
	return project
		? t("Skill change in {{project_name}}", { project_name: project.name })
		: t("Skill change");
};

/** Whether a proposal's skill is a third party's, so it can only come back as a fork. */
export const needsFork = (library: SkillLibrary | undefined, proposal: SkillProposal): boolean =>
	library?.sources.find((s) => s.id === proposal.source)?.trust === "third_party";

/**
 * Why a proposal can't be approved yet, or null when it can. A third
 * party's skill comes back as a fork in your personal source, so it needs
 * a new name first. The holder says when the name is taken.
 */
export const cantApprove = (
	library: SkillLibrary | undefined,
	proposal: SkillProposal,
	fork = ""
): string | null => {
	if (!needsFork(library, proposal)) return null;
	const name = fork.trim();
	if (name === "") {
		return t(
			"This skill is a third party's. Name a fork to approve it into your personal source."
		);
	}
	return name === proposal.name ? t("Give the fork a name of its own.") : null;
};

/** What a proposal's viewer works out from the library, the diff once it came, and the fork's name. */
export type ProposalReview = {
	/** The library source the change goes to. */
	readonly from: SkillSource | undefined;
	/** Whether it needs a fork's name, being a third party's skill. */
	readonly forking: boolean;
	/** Why it can't be approved yet, or null when it can. */
	readonly refusal: string | null;
	/** The tree the change replaces, or null for a new skill. */
	readonly base: TreeHash | null;
	/** What an approve carries, so it can't land on anything else, or undefined before the diff. */
	readonly seen: ProposalSeen | undefined;
	/** "New skill", or the skill it replaces when it didn't start from one, once the diff came. */
	readonly standing: string | null;
};

/**
 * A proposal as its viewer shows it. Once the holder said where the
 * change goes, in the diff's target, that's what counts.
 */
export const proposalReview = (
	library: SkillLibrary | undefined,
	proposal: SkillProposal,
	diff: ProposalDiff | undefined,
	fork: string
): ProposalReview => {
	const target = diff?.target ?? null;
	const aimed = target ? { ...proposal, source: target.source } : proposal;
	const from = library?.sources.find((s) => s.id === aimed.source);
	const base = target ? target.base : proposal.base;
	const standing = !diff
		? null
		: base === null
			? t("New skill")
			: proposal.base === null
				? t("Replaces {{name}}{{value2}}", {
						name: proposal.name,
						value2: from ? t(" in {{name}}", { name: from.name }) : "",
					})
				: null;
	return {
		from,
		forking: needsFork(library, aimed),
		refusal: cantApprove(library, aimed, fork),
		base,
		seen: diff ? { tree: diff.tree ?? proposal.tree, base } : undefined,
		standing,
	};
};

/** What a proposal's viewer says once the device took the answer. */
export const answerDone = (action: ProposalAction, push: boolean): string =>
	action === "reject" ? t("Rejected") : push ? t("Approved and pushed") : t("Approved");

/** Whether tidying a device's global skills would do nothing. */
export const globalsPlanEmpty = (plan: GlobalsPlan): boolean =>
	plan.moves.length +
		plan.links.length +
		plan.backups.length +
		(plan.joins?.length ?? 0) +
		(plan.conflicts?.length ?? 0) ===
	0;

/**
 * The setting that hides one of `cli`'s skills, which stays yours to
 * change: MaxHQ never writes a CLI's settings.
 */
export const hideHint = (cli: Vendor): string | null =>
	({
		claude: t("To hide one, add it to skillOverrides in Claude's settings, or use /skills."),
		codex: t(
			"To hide one, add a [[skills.config]] entry with its path and enabled = false to Codex's config.toml."
		),
		cursor: null,
		acp: null,
	})[cli];

/** The devices whose `maxd` takes skills requests, by name, for a tidy each. */
export const devicesWithSkills = (view: ViewState): DeviceId[] =>
	all(view, "device")
		.filter((device) => offersSkills(view, device.id))
		.sort((a, b) => a.name.localeCompare(b.name))
		.map((device) => device.id);

/** The other projects' skills on `device` that "Add from project" can copy, by project. */
export const otherLocks = (
	view: ViewState,
	project: ProjectId,
	device: DeviceId
): ProjectSkills[] =>
	all(view, "project_skills")
		.filter((p) => p.device === device && p.project !== project && p.lock.skills.length > 0)
		.sort((a, b) => a.project.localeCompare(b.project));
