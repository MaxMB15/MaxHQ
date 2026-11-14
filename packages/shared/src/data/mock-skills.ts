// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The skills of the sample data and of tests. The Mac holds the library:
// two sources of your own and a third party's. MaxHQ has `git-flow` and a
// pinned third-party skill, with `unslop` on for every agent, and uses
// devflow as its pack; Comet has its own devflow installed in the
// checkout, which the scan marks vendored and the pack row too. A scan
// sees the skills still in your global folders, Codex's system skills, a
// claude.ai skill and three plugin skills. Three scenarios add an `unslop`
// whose copies differ, a proposal, and drift with a Cursor agent waiting
// for a restart and a pack install that failed on the devbox. Changes
// live in memory, and each library, project and
// proposal goes out as its own entity the way `maxd` publishes them.

import type {
	AgentPlace,
	DeviceId,
	EffectiveSkill,
	Entity,
	GlobalsPlan,
	LockedSkill,
	PackState,
	ProjectId,
	ProjectSkills,
	ProposalDiff,
	SkillInfo,
	SkillLibrary,
	SkillProposal,
	SkillSource,
	Timestamp,
	Vendor,
} from "../bindings";
import { DEVBOX, MAC, MHQ_WORKSPACE } from "./mock-view";
import type { ActionResult, DataSource, Scenario } from "./source";

/** The skills calls of a data source. */
export type SkillCalls = Pick<
	DataSource,
	| "addSkillSource"
	| "fetchSkillSource"
	| "removeSkillSource"
	| "setProjectSkills"
	| "applySkills"
	| "scanSkills"
	| "skillTreesNeeded"
	| "answerSkillProposal"
	| "skillProposalDiff"
	| "planGlobalSkills"
	| "applyGlobalSkills"
	| "undoGlobalSkills"
	| "moveSkillLibrary"
	| "pushSkillSource"
>;

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

const refused = (reason: string): ActionResult => ({
	status: "ok",
	data: { status: "refused", reason },
});

const notYet = async (): Promise<ActionResult> => refused("Skills aren't set up yet.");

/** The skills calls with no skills behind them, refused the way `maxd` does for now. */
export const noSkills: SkillCalls = {
	addSkillSource: notYet,
	fetchSkillSource: notYet,
	removeSkillSource: notYet,
	setProjectSkills: notYet,
	applySkills: notYet,
	scanSkills: notYet,
	skillTreesNeeded: notYet,
	answerSkillProposal: notYet,
	skillProposalDiff: notYet,
	planGlobalSkills: notYet,
	applyGlobalSkills: notYet,
	undoGlobalSkills: notYet,
	moveSkillLibrary: notYet,
	pushSkillSource: notYet,
};

/** The scenarios the sample skills add. Devbox stays connected in each. */
export const SKILL_SCENARIOS: readonly Scenario[] = [
	{ id: "skills_conflict", label: "Skills conflict" },
	{ id: "skills_proposal", label: "Skills proposal" },
	{ id: "skills_drift", label: "Skills drift" },
];

/** Whether a scenario is one of the skills ones. */
export const isSkillScenario = (scenario: string): boolean =>
	SKILL_SCENARIOS.some((s) => s.id === scenario);

/** Your own skills. */
export const AGENT_SKILLS = "source-agent-skills";
/** devflow's skills. */
export const DEVFLOW = "source-devflow";
/** A third party's skills, pinned. */
export const THIRD_PARTY = "source-third-party";
/** The `git-flow` proposal of the proposal scenario. */
export const GIT_FLOW_PROPOSAL = "proposal-git-flow";

/** The MaxHQ checkout on the Mac. */
const MAC_MAXHQ = "/Users/me/Code/Github/Multi/MaxAIManager";
/** The Comet checkout on the Mac, where the Cursor agent runs. */
const MAC_COMET = "/Users/me/Code/Github/Multi/Comet";
/** The MaxHQ container's checkout on devbox. */
const DEVBOX_MAXHQ = "/home/agent/work/MaxHQ";

const HOME = "/Users/me";
const MIN = 60_000;
const DAY = 24 * 60 * MIN;

const NO_FLAGS = { scripts: false, hooks: false, allowed_tools: false };

const info = (
	source: string,
	name: string,
	description: string,
	over: Partial<SkillInfo> = {}
): SkillInfo => ({
	name,
	description,
	tree: `tree-${name}`,
	path: `skills/${name}`,
	source,
	flags: NO_FLAGS,
	listing_chars: name.length + description.length + 5,
	...over,
});

const sampleSources = (now: Timestamp): SkillSource[] => [
	{
		id: AGENT_SKILLS,
		name: "example/agent-skills",
		kind: { kind: "git", url: "https://github.com/example/agent-skills" },
		default_ref: null,
		trust: "own",
		commit: "4f1c2a9",
		fetched_at: now - 2 * 60 * MIN,
		problem: null,
		skills: [
			info(
				AGENT_SKILLS,
				"git-flow",
				"Branch strategy with main, dev, feature, release and hotfix."
			),
			info(
				AGENT_SKILLS,
				"native-rendering-discipline",
				"Rules for native rendering code that calls C graphics libraries.",
				{ flags: { ...NO_FLAGS, allowed_tools: true } }
			),
			info(AGENT_SKILLS, "grill-me", "Ask hard questions about a plan before it starts."),
			info(AGENT_SKILLS, "find-skills", "Find a skill in the library that fits the task."),
			info(
				AGENT_SKILLS,
				"propose-skill-change",
				"Write a changed skill to the proposals folder for review."
			),
		],
	},
	{
		id: DEVFLOW,
		name: "example/devflow",
		kind: { kind: "git", url: "https://github.com/example/devflow" },
		default_ref: null,
		trust: "own",
		commit: "8be68f9",
		fetched_at: now - DAY,
		problem: null,
		pack: PACK_TREE,
		skills: [
			info(DEVFLOW, "unslop", "Cut AI tells from any writing."),
			info(DEVFLOW, "df-plan", "Write a plan in the devflow format.", {
				flags: { ...NO_FLAGS, scripts: true },
			}),
		],
	},
	{
		id: THIRD_PARTY,
		name: "example/third-party-skills",
		kind: { kind: "git", url: "https://github.com/example/third-party-skills" },
		default_ref: "v1.2.0",
		trust: "third_party",
		commit: "a07d3e1",
		fetched_at: now - 3 * DAY,
		problem: null,
		skills: [
			info(
				THIRD_PARTY,
				"screen-reader-check",
				"Check a view the way a screen reader reads it.",
				{
					flags: { scripts: true, hooks: true, allowed_tools: false },
				}
			),
		],
	},
];

/** The devflow source's whole tree, as MaxHQ's pack. */
export const PACK_TREE = "c41e9b0752d84f6a9e3b1c07d2a58f4e6b9d0c3a7e5f21b8d4c6a9e0f3b7d152";

/** The skills every agent gets globally. */
const ALWAYS = ["unslop", "propose-skill-change"];

const locked = (source: SkillSource, name: string, pinned = false): LockedSkill => {
	const skill = source.skills.find((s) => s.name === name)!;
	return {
		name,
		source: source.id,
		path: skill.path,
		commit: source.commit,
		tree: skill.tree,
		pinned,
	};
};

const checkout = (path: string, pack: PackState | null = null) => ({
	path,
	applied: true,
	drift: [],
	needs_restart: false,
	pack,
});

const INSTALLED: PackState = { state: "installed", tree: PACK_TREE };

const FAILED: PackState = {
	state: "failed",
	tree: PACK_TREE,
	lines: [
		"devflow: installing the Python tools",
		"error: uv isn't on the PATH. Install it from https://docs.astral.sh/uv/",
		"The installer stopped with exit status: 1.",
	],
};

const sampleProjects = (sources: readonly SkillSource[]): ProjectSkills[] => {
	const [mine, devflow, theirs] = sources;
	const maxhq = {
		skills: [locked(mine!, "git-flow"), locked(theirs!, "screen-reader-check", true)],
		committed: false,
		pack: { source: devflow!.id, commit: devflow!.commit!, tree: PACK_TREE },
	};
	return [
		{
			project: "proj-maxhq",
			device: MAC,
			lock: maxhq,
			checkouts: [checkout(MAC_MAXHQ, INSTALLED)],
		},
		{
			project: "proj-maxhq",
			device: DEVBOX,
			lock: maxhq,
			checkouts: [checkout(DEVBOX_MAXHQ, INSTALLED)],
		},
		{
			project: "proj-comet",
			device: MAC,
			lock: { skills: [locked(mine!, "grill-me")], committed: true },
			checkouts: [checkout(MAC_COMET, { state: "vendored" })],
		},
	];
};

const proposal = (now: Timestamp): SkillProposal => ({
	id: GIT_FLOW_PROPOSAL,
	device: MAC,
	project: "proj-maxhq",
	agent: "agent-release",
	name: "git-flow",
	base: "tree-git-flow",
	tree: "tree-git-flow-proposed",
	checkout: MAC_MAXHQ,
	created_at: now - 6 * MIN,
	flags: NO_FLAGS,
	source: AGENT_SKILLS,
});

/** What the `git-flow` proposal changes: a step in its instructions and a new reference file. */
const PROPOSAL_DIFF: ProposalDiff = {
	files: [
		{
			path: "SKILL.md",
			status: "modified",
			additions: 2,
			deletions: 1,
			patch: [
				"@@ -4,5 +4,6 @@ description: Branch, release and hotfix with git-flow.",
				" ",
				" 1. Start a feature branch from dev.",
				"-2. Merge it back when it's done.",
				"+2. Rebase it on dev, then merge it back when it's done.",
				"+3. Read references/release.md before a release.",
				" ",
				"",
			].join("\n"),
		},
		{
			path: "references/release.md",
			status: "added",
			additions: 3,
			deletions: 0,
			patch: [
				"@@ -0,0 +1,3 @@",
				"+# Releases",
				"+",
				"+Cut the release branch from dev and tag it on main.",
				"",
			].join("\n"),
		},
	],
	flags: NO_FLAGS,
};

const effective = (
	cli: Vendor,
	name: string,
	place: EffectiveSkill["place"],
	path: string,
	over: Partial<EffectiveSkill> = {}
): EffectiveSkill => ({
	name,
	description: null,
	path: `${path}/${name}/SKILL.md`,
	scope: null,
	enabled: true,
	display_name: null,
	short_description: null,
	user_invocation_only: null,
	user_invocable: null,
	place,
	cli,
	mark: null,
	...over,
});

/** What each CLI sees in a checkout of `project` at `root`, with `unslop` differing when `conflict`. */
const scanOf = (
	library: SkillLibrary,
	project: ProjectSkills | undefined,
	root: string,
	conflict: boolean
): EffectiveSkill[] => {
	const described = (name: string) => {
		for (const source of library.sources) {
			const found = source.skills.find((s) => s.name === name);
			if (found) return { description: found.description };
		}
		return {};
	};
	const locked = project?.lock.skills.map((s) => s.name) ?? [];
	const claude = `${HOME}/.claude/skills`;
	const agents = `${HOME}/.agents/skills`;
	const codex = `${HOME}/.codex/skills`;
	const skills: EffectiveSkill[] = [];
	for (const cli of ["claude", "codex", "cursor"] as const) {
		const projectFolder = `${root}/.${cli === "codex" ? "agents" : cli}/skills`;
		for (const name of locked) {
			// Claude runs a global skill over a project one of the same name.
			const shadowed = cli === "claude" && name === "git-flow";
			skills.push(
				effective(cli, name, "project", projectFolder, {
					...described(name),
					mark: shadowed ? "shadowed" : null,
				})
			);
		}
		for (const name of library.always) {
			skills.push(
				effective(cli, name, "always", cli === "claude" ? claude : agents, {
					...described(name),
					mark: conflict && name === "unslop" && cli !== "claude" ? "conflict" : null,
				})
			);
		}
	}
	// What's still in your global folders, read before any tidy.
	skills.push(
		effective("claude", "git-flow", "global", claude, described("git-flow")),
		effective("claude", "native-rendering-discipline", "global", claude, {
			...described("native-rendering-discipline"),
		}),
		effective("codex", "git-flow", "global", agents, {
			...described("git-flow"),
			mark: "same",
		}),
		effective("codex", "native-rendering-discipline", "global", agents, {
			...described("native-rendering-discipline"),
			mark: "same",
		}),
		effective("cursor", "native-rendering-discipline", "global", agents, {
			...described("native-rendering-discipline"),
		})
	);
	if (conflict) {
		skills.push(
			effective("codex", "unslop", "global", codex, {
				description: "Cut AI tells from writing. An older copy.",
				mark: "conflict",
			})
		);
	}
	skills.push(
		effective("claude", "brand-voice", "global", `${claude}/synced`, {
			description: "Write in the team's voice.",
			scope: "synced",
		}),
		...["imagegen", "openai-docs", "skill-creator"].map((name) =>
			effective("codex", name, "system", `${codex}/.system`, { scope: "system" })
		),
		...["frontend-design:frontend-design", "context7:docs", "superpowers:brainstorming"].map(
			(name) =>
				effective(
					"claude",
					name,
					"plugin",
					`${HOME}/.claude/plugins/${name.split(":")[0]}`,
					{
						scope: "plugin",
					}
				)
		)
	);
	if (root === MAC_COMET) {
		// devflow links its own skills into the checkout.
		for (const name of ["df-plan", "unslop"]) {
			skills.push(
				effective("claude", name, "project", `${root}/.claude/skills`, {
					...described(name),
					mark: "vendored",
				})
			);
		}
	}
	return skills;
};

/** The sample skills, as calls and the entities that show them. */
export type MockSkills = {
	readonly calls: SkillCalls;
	/** Each skill entity as it stands in `scenario`. */
	readonly entities: (scenario: string) => Entity[];
};

/** Where each sample workspace's checkout is, by workspace. */
const WORKSPACE_PATHS: Record<string, { device: DeviceId; path: string; project: ProjectId }> = {
	"ws-mac-maxhq": { device: MAC, path: MAC_MAXHQ, project: "proj-maxhq" },
	"ws-mac-comet": { device: MAC, path: MAC_COMET, project: "proj-comet" },
	[MHQ_WORKSPACE]: { device: DEVBOX, path: DEVBOX_MAXHQ, project: "proj-maxhq" },
};

/**
 * The sample skills, kept in memory. `scenario` says which scenario the
 * sample data shows, and `changed` runs after each change, so the caller
 * can publish the new entities.
 */
export const mockSkills = ({
	now,
	scenario,
	changed = () => {},
}: {
	now: () => Timestamp;
	scenario: () => string;
	changed?: () => void;
}): MockSkills => {
	const library: SkillLibrary = { device: MAC, sources: sampleSources(now()), always: ALWAYS };
	const projects = new Map(
		sampleProjects(library.sources).map((p) => [`${p.project}/${p.device}`, p])
	);
	const proposals = new Map<string, SkillProposal>([[GIT_FLOW_PROPOSAL, proposal(now())]]);
	let tidied = false;
	let sourceCount = 0;
	/** The projects a Restore or Apply put back since the scenario showed drift. */
	const restored = new Set<string>();

	const projectAt = (project: ProjectId, device: DeviceId) =>
		projects.get(`${project}/${device}`);

	/**
	 * The scenario's touches on a project: drift on Comet, with its Cursor
	 * agent waiting, and a failed pack install on the devbox. A Restore
	 * clears the drift and an Apply runs the install again, as real ones
	 * do, and the agent still waits.
	 */
	const shown = (skills: ProjectSkills, at: string): ProjectSkills => {
		const key = `${skills.project}/${skills.device}`;
		if (at === "skills_drift" && skills.device === DEVBOX && !restored.has(key)) {
			return { ...skills, checkouts: skills.checkouts.map((c) => ({ ...c, pack: FAILED })) };
		}
		return at === "skills_drift" && skills.project === "proj-comet"
			? {
					...skills,
					checkouts: skills.checkouts.map((c) => ({
						...c,
						drift: restored.has(`${skills.project}/${skills.device}`)
							? []
							: ["grill-me"],
						needs_restart: true,
						outside: [".claude"],
					})),
				}
			: skills;
	};

	const entities = (at: string): Entity[] => [
		{ entity: "skill_library", ...library },
		...[...projects.values()].map((p): Entity => ({
			entity: "project_skills",
			...shown(p, at),
		})),
		...(at === "skills_proposal" ? [...proposals.values()] : []).map((p): Entity => ({
			entity: "skill_proposal",
			...p,
		})),
	];

	const onHolder = (device: DeviceId) =>
		device === library.device ? null : refused("This device doesn't hold the skill library.");

	const PLAN: GlobalsPlan = {
		hash: "plan-mac-1",
		moves: [`${HOME}/.claude/skills/git-flow`, `${HOME}/.agents/skills/git-flow`],
		links: [`${HOME}/.claude/skills/unslop`, `${HOME}/.agents/skills/unslop`],
		backups: [`${HOME}/.codex/skills/unslop`],
		joins: [
			{
				name: "owner/skills",
				url: "https://github.com/owner/skills",
				paths: [`${HOME}/.agents/skills/grill-me`],
			},
		],
		conflicts: [
			{
				name: "review",
				paths: [`${HOME}/.claude/skills/review`, `${HOME}/.codex/skills/review`],
			},
		],
	};

	const calls: SkillCalls = {
		addSkillSource: async (device, name, origin, defaultRef, trust) => {
			const not = onHolder(device);
			if (not) return not;
			if (library.sources.some((s) => s.name === name)) {
				return refused(`The library already has a source called ${name}.`);
			}
			sourceCount += 1;
			library.sources = [
				...library.sources,
				{
					id: `source-added-${sourceCount}`,
					name,
					kind: origin,
					default_ref: defaultRef,
					trust,
					commit: origin.kind === "git" ? "0000000" : null,
					fetched_at: now(),
					problem: null,
					skills: [],
				},
			];
			changed();
			return DONE;
		},
		fetchSkillSource: async (device, id) => {
			const not = onHolder(device);
			if (not) return not;
			if (!library.sources.some((s) => s.id === id)) return refused("That source is gone.");
			library.sources = library.sources.map((s) =>
				s.id === id ? { ...s, fetched_at: now(), problem: null } : s
			);
			changed();
			return DONE;
		},
		removeSkillSource: async (device, id) => {
			const not = onHolder(device);
			if (not) return not;
			const used = [...projects.values()].some(
				(p) => p.lock.skills.some((s) => s.source === id) || p.lock.pack?.source === id
			);
			if (used)
				return refused("A project uses skills from this source. Take them out first.");
			library.sources = library.sources.filter((s) => s.id !== id);
			changed();
			return DONE;
		},
		setProjectSkills: async (device, project, lock) => {
			const always = lock.skills.find((s) => library.always.includes(s.name));
			if (always) return refused(`Every agent already gets ${always.name}.`);
			const before = projectAt(project, device);
			projects.set(`${project}/${device}`, {
				project,
				device,
				lock,
				checkouts: (before?.checkouts ?? []).map((c) => ({ ...c, applied: false })),
			});
			changed();
			return DONE;
		},
		applySkills: async (device, project) => {
			const found = projectAt(project, device);
			if (!found) return refused("This project has no skills on this device yet.");
			const pack = (c: ProjectSkills["checkouts"][number]): PackState | null => {
				if (c.pack?.state === "vendored") return c.pack;
				return found.lock.pack ? { state: "installed", tree: found.lock.pack.tree } : null;
			};
			projects.set(`${project}/${device}`, {
				...found,
				checkouts: found.checkouts.map((c) => ({
					...c,
					applied: true,
					drift: [],
					pack: pack(c),
				})),
			});
			restored.add(`${project}/${device}`);
			changed();
			return DONE;
		},
		scanSkills: async (device, place: AgentPlace) => {
			const at =
				place.type === "workspace"
					? WORKSPACE_PATHS[place.workspace]
					: Object.values(WORKSPACE_PATHS).find((w) => w.path === place.path);
			const root = at?.path ?? (place.type === "folder" ? place.path : "/work/repo");
			const project = at && at.device === device ? projectAt(at.project, device) : undefined;
			const conflict = scenario() === "skills_conflict" && !tidied;
			return {
				status: "ok",
				data: { status: "skills", skills: scanOf(library, project, root, conflict) },
			};
		},
		skillTreesNeeded: async () => ({
			status: "ok",
			data: { status: "skill_trees", trees: [] },
		}),
		answerSkillProposal: async (id, action, _push, fork) => {
			const found = proposals.get(id);
			if (!found) {
				return { status: "error", error: { kind: "not_found", what: "This proposal" } };
			}
			const source = library.sources.find((s) => s.id === found.source);
			if (action === "approve" && source?.trust === "third_party" && !fork?.trim()) {
				return refused(
					"This skill is a third party's. Name a fork to approve it into your personal source."
				);
			}
			proposals.delete(id);
			changed();
			return DONE;
		},
		skillProposalDiff: async (id) =>
			proposals.has(id)
				? { status: "ok", data: { status: "skill_proposal_diff", diff: PROPOSAL_DIFF } }
				: { status: "error", error: { kind: "not_found", what: "This proposal" } },
		planGlobalSkills: async (device) =>
			device === MAC
				? {
						status: "ok",
						data: { status: "skills_globals_plan", plan: { ...PLAN, undo: tidied } },
					}
				: refused("Container config folders are MaxHQ's, so there's nothing to tidy."),
		applyGlobalSkills: async (device, planHash, picks) => {
			if (device !== MAC || planHash !== PLAN.hash) {
				return refused("The global skills changed since the plan. Look at it again.");
			}
			const copies = PLAN.conflicts!.flatMap((c) => c.paths);
			const stray = picks.find((pick) => !copies.includes(pick));
			if (stray) return refused(`${stray} isn't a copy you can pick.`);
			tidied = true;
			return DONE;
		},
		undoGlobalSkills: async (device) => {
			if (device !== MAC || !tidied) return refused("There's no tidy to undo.");
			tidied = false;
			return DONE;
		},
		moveSkillLibrary: async (device) => {
			if (device === library.device) return DONE;
			if (library.sources.some((s) => s.unpushed)) {
				return refused(
					"The holder has approved skill changes it hasn't pushed. Push the library's approvals first."
				);
			}
			library.device = device;
			changed();
			return DONE;
		},
		pushSkillSource: async (device, id) => {
			const not = onHolder(device);
			if (not) return not;
			library.sources = library.sources.map((s) =>
				s.id === id ? { ...s, unpushed: false } : s
			);
			changed();
			return DONE;
		},
	};

	return { calls, entities };
};
