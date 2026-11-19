// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the `@`, `#` and `$` menus offer, and what a pick puts in the box.
// `@` finds files and folders in the agent's workspace, other agents in a
// group of their own, and notes in another, the agent's own project's
// first. A field that takes no files leaves them out, and one with no
// agent of its own finds no pull requests either. `#` finds pull requests
// and issues in the agent's repository, and with the caret in a shown
// `#123` it offers Keep as text too. `$` lists the skills the agent's CLI
// offers there, from the last scan. A lookup waits for a short pause in
// typing, an answer for a word that has changed since is dropped, and the
// last answer narrows while the next is on its way. Each app draws the
// menu its own way.

import { useEffect, useMemo, useRef, useState } from "react";

import type { Agent, AgentId, EffectiveSkill } from "../bindings";
import type { DataSource, GitItemSearch, PathMatch } from "../data/source";
import { formatList, t } from "../i18n";
import type { AgentRef, GitItemRef, Segment } from "../pills/draft";
import { rankGitItems } from "../pills/git-matches";
import { detectTrigger, type Trigger } from "../pills/trigger";
import type { SkillFeed } from "../skills/feed";
import { skillHint, skillLabel, skillMenu } from "../skills/menu";
import { sentence } from "../status/format";
import type { NoteChoice } from "../view/notes";

/** How long typing pauses before a menu looks its word up. */
export const MENU_DELAY_MS = 180;

const MAX_AGENTS = 5;

const MAX_NOTES = 6;

const MAX_GIT_ITEMS = 8;

/** One thing a menu offers. */
export type MenuOption =
	| { readonly kind: "path"; readonly path: string; readonly folder: boolean }
	| { readonly kind: "agent"; readonly agent: Agent }
	| { readonly kind: "note"; readonly note: NoteChoice }
	| { readonly kind: "git"; readonly item: GitItemRef }
	| { readonly kind: "keep"; readonly number: number }
	| { readonly kind: "skill"; readonly skill: EffectiveSkill };

/** The menus that look their word up on the device. */
type LookupKind = Exclude<Trigger["kind"], "skill">;

/** What the last lookup found, for which kind of menu. */
export type MenuFound =
	| { readonly kind: "path"; readonly paths: readonly PathMatch[] }
	| { readonly kind: "git-item"; readonly search: GitItemSearch }
	| { readonly kind: "failed"; readonly for: LookupKind };

/** A menu's options and what it says while it has none. */
export type MenuContents = {
	readonly options: readonly MenuOption[];
	/** True until the first answer for this kind of menu comes back. */
	readonly loading: boolean;
	/** Why there's nothing to find, such as an agent with no GitHub repository. */
	readonly notice: string | null;
};

/** Whether each character of the query shows up in the text, in order. */
const fuzzy = (text: string, query: string): boolean => {
	let at = 0;
	for (const char of text.toLowerCase()) if (char === query[at]) at += 1;
	return at === query.length;
};

/** What a menu works from, besides the lookup's answer. */
export type MenuInputs = {
	/** The agent whose folder and repo the lookups search, and whom `@` leaves out, or null. */
	readonly agent: AgentId | null;
	/** The agents the `@` menu can name. */
	readonly agents: readonly Agent[];
	/** The notes that match the `@` menu's word. */
	readonly notes?: readonly NoteChoice[];
	/** Whether `@` finds files and folders. */
	readonly files?: boolean;
	/** The word the menu is for, or null while none is open. */
	readonly trigger: Pick<Trigger, "kind" | "query"> | null;
	/** The shown number the caret is in, if it's in one. */
	readonly keep: number | null;
	/** What the `$` menu lists from. */
	readonly skills: SkillFeed;
};

/** Whether the open menu looks its word up on the agent's device. */
export const menuLooks = ({ agent, files = true, trigger }: MenuInputs): boolean =>
	agent !== null && (trigger?.kind === "git-item" || (trigger?.kind === "path" && files));

/** What the open menu offers, from the last lookup's answer. */
export const menuContents = (inputs: MenuInputs, found: MenuFound | null): MenuContents => {
	const { agent, agents, notes = [], trigger, keep, skills } = inputs;
	const kind = trigger?.kind ?? null;
	const query = trigger?.query ?? "";
	const looks = menuLooks(inputs);
	if (kind === null) return { options: [], loading: false, notice: null };
	if (kind === "skill") {
		return {
			options: skillMenu(skills.skills ?? [], skills.cli, query).map((skill): MenuOption => ({
				kind: "skill",
				skill,
			})),
			loading: skills.loading,
			notice: skills.notice,
		};
	}
	const failed = looks && found?.kind === "failed" && found.for === kind;
	const loading = looks && (found === null || (found.kind !== kind && !failed));
	if (kind === "path") {
		const wanted = query.toLowerCase();
		const paths = looks && found?.kind === "path" ? found.paths : [];
		const options: MenuOption[] = [
			...paths
				.filter((match) => fuzzy(match.path, wanted))
				.map((match): MenuOption => ({
					kind: "path",
					path: match.path,
					folder: match.kind === "folder",
				})),
			...agents
				.filter((each) => each.id !== agent && each.title.toLowerCase().includes(wanted))
				.slice(0, MAX_AGENTS)
				.map((each): MenuOption => ({ kind: "agent", agent: each })),
			...notes.slice(0, MAX_NOTES).map((note): MenuOption => ({ kind: "note", note })),
		];
		return {
			options,
			loading,
			notice: failed ? t("Couldn't search this agent's folder") : null,
		};
	}
	const search = looks && found?.kind === "git-item" ? found.search : null;
	const items = search ? rankGitItems({ items: search.items, query, limit: MAX_GIT_ITEMS }) : [];
	return {
		options: [
			...items.map((item): MenuOption => ({ kind: "git", item })),
			...(keep === null ? [] : [{ kind: "keep", number: keep } as const]),
		],
		loading,
		notice: failed ? t("Couldn't reach GitHub") : (search?.unavailable ?? null),
	};
};

/** What the open menu offers, looking its word up after a pause in typing. */
export const useMenuContents = (inputs: MenuInputs & { source: DataSource }): MenuContents => {
	const { source, agent, agents, notes, files, trigger, keep, skills } = inputs;
	const [found, setFound] = useState<MenuFound | null>(null);
	const asked = useRef(0);
	const kind = trigger?.kind ?? null;
	const query = trigger?.query ?? "";
	const looks = menuLooks(inputs);

	useEffect(() => {
		if (!looks || agent === null || (kind !== "path" && kind !== "git-item")) return;
		const ask = ++asked.current;
		const timer = setTimeout(() => {
			const answer: Promise<MenuFound> =
				kind === "path"
					? source.findPaths(agent, query).then((paths) => ({ kind, paths }))
					: source.findGitItems(agent, query).then((search) => ({ kind, search }));
			answer.then(
				(next) => {
					if (asked.current === ask) setFound(next);
				},
				() => {
					if (asked.current === ask) setFound({ kind: "failed", for: kind });
				}
			);
		}, MENU_DELAY_MS);
		return () => clearTimeout(timer);
	}, [agent, kind, looks, query, source]);

	return useMemo(
		() =>
			menuContents(
				{ agent, agents, notes, files, trigger: kind && { kind, query }, keep, skills },
				found
			),
		[agent, agents, files, found, keep, kind, notes, query, skills]
	);
};

/** Names the word at the caret that a menu is for, so closing it can keep it shut. */
export const menuWord = (text: string, caret: number | null): string | null => {
	const trigger = caret === null ? null : detectTrigger(text, caret);
	return trigger
		? `${trigger.rangeStart}:${text.slice(trigger.rangeStart, trigger.rangeEnd)}`
		: null;
};

/** What a pick does to the box. */
export type MenuPick = {
	/** What goes in place of the menu's word, or null when nothing does. */
	readonly segments: readonly Segment[] | null;
	/** Whether a space follows what goes in. */
	readonly spaced: boolean;
	/** A pull request or issue the box now knows, to show as a pill. */
	readonly seed: GitItemRef | null;
	/** A number to keep as text, which also shuts the menu. */
	readonly keep: number | null;
};

const NOTHING: MenuPick = { segments: null, spaced: false, seed: null, keep: null };

/**
 * What picking `option` does. `onPath` makes a picked file or folder's
 * segments, or is null when the box takes none, and `agentPill` makes an
 * agent's pill, or null for none.
 */
export const menuPick = (
	option: MenuOption,
	{
		onPath,
		agentPill,
	}: {
		readonly onPath: ((path: string) => readonly Segment[]) | null;
		readonly agentPill: (agent: Agent) => AgentRef | null;
	}
): MenuPick => {
	switch (option.kind) {
		case "path": {
			const segments = onPath?.(`${option.path}${option.folder ? "/" : ""}`) ?? [];
			return segments.length > 0 ? { ...NOTHING, segments } : NOTHING;
		}
		case "agent": {
			const pill = agentPill(option.agent);
			return pill ? { ...NOTHING, segments: [{ type: "pill", pill }] } : NOTHING;
		}
		case "note":
			return { ...NOTHING, segments: [{ type: "pill", pill: option.note.ref }] };
		case "git":
			return {
				...NOTHING,
				segments: [{ type: "text", text: `#${option.item.number}` }],
				spaced: true,
				seed: option.item,
			};
		case "keep":
			return { ...NOTHING, keep: option.number };
		case "skill":
			// The skill goes in as `$name`.
			return {
				...NOTHING,
				segments: [{ type: "text", text: `$${option.skill.name}` }],
				spaced: true,
			};
	}
};

/** An option's words: its label, and a detail to show after it. */
export const optionText = (option: MenuOption): { label: string; detail?: string } => {
	switch (option.kind) {
		case "path":
			return { label: option.folder ? `${option.path}/` : option.path };
		case "agent":
			return { label: option.agent.title };
		case "note":
			return {
				label: option.note.shown,
				...(option.note.own ? {} : { detail: option.note.projectName }),
			};
		case "git":
			return { label: option.item.title, detail: `#${option.item.number}` };
		case "keep":
			return { label: t("Keep #{{number}} as text", { number: String(option.number) }) };
		case "skill":
			return { label: skillLabel(option.skill), detail: skillHint(option.skill) };
	}
};

/** The heading over each kind of option. */
export const groupName = (kind: MenuOption["kind"]): string =>
	({
		path: () => t("Files and folders"),
		agent: () => t("Agents"),
		note: () => t("Notes"),
		git: () => t("Pull requests and issues"),
		keep: () => t("This number"),
		skill: () => t("Skills"),
	})[kind]();

/** A run of options of one kind, with the index its first has in the whole list. */
export type MenuGroup = {
	readonly kind: MenuOption["kind"];
	readonly start: number;
	readonly options: MenuOption[];
};

/** The options in runs of one kind, in their order. */
export const menuGroups = (options: readonly MenuOption[]): MenuGroup[] => {
	const groups: MenuGroup[] = [];
	options.forEach((option, index) => {
		const last = groups.at(-1);
		if (last?.kind === option.kind) last.options.push(option);
		else groups.push({ kind: option.kind, start: index, options: [option] });
	});
	return groups;
};

/** What a field's `@` menu finds, for the words that name it. */
export type MenuFinds = { readonly files: boolean; readonly notes: boolean };

export const FINDS_EVERYTHING: MenuFinds = { files: true, notes: true };

/** What the `@` menu finds, such as "files, folders, agents and notes", or with "or". */
const pathFinds = (finds: MenuFinds, type: "conjunction" | "disjunction"): string =>
	formatList(
		[
			...(finds.files ? [t("files"), t("folders")] : []),
			t("agents"),
			...(finds.notes ? [t("notes")] : []),
		],
		{ type }
	);

/** What an open menu with no options says, or null while it has some. */
export const menuStatus = (
	kind: Trigger["kind"],
	{ options, loading, notice }: MenuContents,
	finds: MenuFinds = FINDS_EVERYTHING
): string | null => {
	if (options.length > 0) return null;
	if (notice !== null) return notice;
	if (loading) return t("Searching");
	switch (kind) {
		case "path":
			return t("No {{pathFinds}} match", { pathFinds: pathFinds(finds, "disjunction") });
		case "git-item":
			return t("No pull requests or issues match");
		case "skill":
			return t("No skills match");
	}
};

/** What a screen reader calls the open menu. */
export const menuLabel = (kind: Trigger["kind"], finds: MenuFinds = FINDS_EVERYTHING): string => {
	switch (kind) {
		case "path":
			return sentence(pathFinds(finds, "conjunction"));
		case "git-item":
			return t("Pull requests and issues");
		case "skill":
			return t("Skills");
	}
};
