// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The `@`, `#` and `$` menus above the text box, or below it in a field
// at the top of the window. `@` finds files and
// folders in the agent's workspace, other agents in a group of their own,
// and notes in another, the agent's own project's first. A field that takes
// no files leaves them out, and one with no agent of its own finds no pull
// requests either. `#` finds pull
// requests and issues in the agent's repository, and
// with the caret in a drawn `#123` it offers Keep as text too. `$` lists
// the skills the agent's CLI offers there, from the last scan. A lookup
// waits for a short pause in typing, an answer for a word that has changed
// since is dropped, and the last answer narrows while the next is on its
// way.

import type { Agent, AgentId, EffectiveSkill } from "@maxhq/shared/bindings";
import type { DataSource, GitItemSearch, PathMatch } from "@maxhq/shared/data/source";
import type { GitItemRef } from "@maxhq/shared/pills/draft";
import { rankGitItems } from "@maxhq/shared/pills/git-matches";
import type { Trigger } from "@maxhq/shared/pills/trigger";
import type { SkillFeed } from "@maxhq/shared/skills/feed";
import { skillHint, skillLabel, skillMenu } from "@maxhq/shared/skills/menu";
import { sentence } from "@maxhq/shared/status/format";
import type { NoteChoice } from "@maxhq/shared/view/notes";
import { File, Folder, Sparkles, StickyNote, Type } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { VendorIcon } from "@/agent-view/icons";
import { cn } from "@/lib/utils";
import { GitIcon } from "@/pills/pill";
import { t as translate, useI18n, formatList } from "@maxhq/shared/i18n";

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

/** What the last lookup found, for which kind of menu. */
type Found =
	| { readonly kind: "path"; readonly paths: readonly PathMatch[] }
	| { readonly kind: "git-item"; readonly search: GitItemSearch }
	| { readonly kind: "failed"; readonly for: LookupKind };

/** The menus that look their word up on the device. */
type LookupKind = Exclude<Trigger["kind"], "skill">;

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

/**
 * What the open menu offers. `agent` is the agent whose folder and repo
 * the lookups search, and whom `@` leaves out, or null for none. `agents`
 * are the agents the `@` menu can name, `notes` the notes that match its
 * word, `files` whether it finds files and folders, `keep` is the drawn
 * number the caret is in, if it's in one, and `skills` is what the `$`
 * menu lists from.
 */
export const useMenuContents = ({
	source,
	agent,
	agents,
	notes = [],
	files = true,
	trigger,
	keep,
	skills,
}: {
	source: DataSource;
	agent: AgentId | null;
	agents: readonly Agent[];
	notes?: readonly NoteChoice[];
	files?: boolean;
	trigger: Trigger | null;
	keep: number | null;
	skills: SkillFeed;
}): MenuContents => {
	const [found, setFound] = useState<Found | null>(null);
	const asked = useRef(0);
	const kind = trigger?.kind ?? null;
	const query = trigger?.query ?? "";
	// Whether the open menu looks its word up on the agent's device.
	const looks = agent !== null && (kind === "git-item" || (kind === "path" && files));

	useEffect(() => {
		if (!looks || agent === null) return;
		const ask = ++asked.current;
		const timer = setTimeout(() => {
			const answer: Promise<Found> =
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

	return useMemo((): MenuContents => {
		if (kind === null) return { options: [], loading: false, notice: null };
		if (kind === "skill") {
			return {
				options: skillMenu(skills.skills ?? [], skills.cli, query).map(
					(skill): MenuOption => ({ kind: "skill", skill })
				),
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
					.filter(
						(each) => each.id !== agent && each.title.toLowerCase().includes(wanted)
					)
					.slice(0, MAX_AGENTS)
					.map((each): MenuOption => ({ kind: "agent", agent: each })),
				...notes.slice(0, MAX_NOTES).map((note): MenuOption => ({ kind: "note", note })),
			];
			return {
				options,
				loading,
				notice: failed ? translate("Couldn't search this agent's folder") : null,
			};
		}
		const search = looks && found?.kind === "git-item" ? found.search : null;
		const items = search
			? rankGitItems({ items: search.items, query, limit: MAX_GIT_ITEMS })
			: [];
		return {
			options: [
				...items.map((item): MenuOption => ({ kind: "git", item })),
				...(keep === null ? [] : [{ kind: "keep", number: keep } as const]),
			],
			loading,
			notice: failed ? translate("Couldn't reach GitHub") : (search?.unavailable ?? null),
		};
	}, [agent, agents, found, keep, kind, looks, notes, query, skills]);
};

/** The element ID of a menu's option, for `aria-activedescendant`. */
export const optionId = (menu: string, index: number) =>
	translate("{{menu}}-option-{{index}}", { menu: menu, index: index });

const ROW =
	"flex h-7 w-full cursor-default items-center gap-2 rounded-[5px] px-2 text-left text-[12.5px] text-foreground";

const ICON = "size-3.5 shrink-0 text-muted-foreground";

const optionLook = (option: MenuOption): { icon: ReactNode; label: string; detail?: string } => {
	switch (option.kind) {
		case "path":
			return {
				icon: option.folder ? (
					<Folder aria-hidden className={ICON} />
				) : (
					<File aria-hidden className={ICON} />
				),
				label: option.folder ? `${option.path}/` : option.path,
			};
		case "agent":
			return {
				icon: <VendorIcon vendor={option.agent.vendor} className={ICON} />,
				label: option.agent.title,
			};
		case "note":
			return {
				icon: <StickyNote aria-hidden className={ICON} />,
				label: option.note.shown,
				...(option.note.own ? {} : { detail: option.note.projectName }),
			};
		case "git":
			return {
				icon: (
					<GitIcon
						kind={option.item.kind}
						state={option.item.state}
						aria-hidden
						className={ICON}
					/>
				),
				label: option.item.title,
				detail: `#${option.item.number}`,
			};
		case "keep":
			return {
				icon: <Type aria-hidden className={ICON} />,
				label: translate("Keep #{{number}} as text", { number: String(option.number) }),
			};
		case "skill":
			return {
				icon: <Sparkles aria-hidden className={ICON} />,
				label: skillLabel(option.skill),
				detail: skillHint(option.skill),
			};
	}
};

const GROUPS: Record<MenuOption["kind"], string> = {
	get path() {
		return translate("Files and folders");
	},
	agent: "Agents",
	note: "Notes",
	get git() {
		return translate("Pull requests and issues");
	},
	get keep() {
		return translate("This number");
	},
	skill: "Skills",
};

/** What a field's `@` menu finds, for the words that name it. */
export type MenuFinds = { readonly files: boolean; readonly notes: boolean };

const EVERYTHING: MenuFinds = { files: true, notes: true };

/** What the `@` menu finds, such as "files, folders, agents and notes", or with "or". */
const pathFinds = (finds: MenuFinds, type: "conjunction" | "disjunction"): string =>
	formatList(
		[
			...(finds.files ? [translate("files"), translate("folders")] : []),
			translate("agents"),
			...(finds.notes ? [translate("notes")] : []),
		],
		{ type }
	);

const EMPTY: Record<Exclude<Trigger["kind"], "path">, string> = {
	get "git-item"() {
		return translate("No pull requests or issues match");
	},
	get skill() {
		return translate("No skills match");
	},
};

const LIST_LABEL: Record<Exclude<Trigger["kind"], "path">, string> = {
	get "git-item"() {
		return translate("Pull requests and issues");
	},
	skill: "Skills",
};

/**
 * The open menu, as a list box the text box points into. It opens above
 * the box unless `below` is set, and `finds` says what its `@` menu finds.
 */
export const ComposerMenu = ({
	id,
	kind,
	contents,
	active,
	onActive,
	onPick,
	below = false,
	finds = EVERYTHING,
}: {
	id: string;
	kind: Trigger["kind"];
	contents: MenuContents;
	active: number;
	onActive: (index: number) => void;
	onPick: (option: MenuOption) => void;
	below?: boolean;
	finds?: MenuFinds;
}) => {
	useI18n();
	const { options, loading, notice } = contents;
	const list = useRef<HTMLDivElement>(null);
	useEffect(() => {
		list.current?.ownerDocument
			.getElementById(optionId(id, active))
			?.scrollIntoView({ block: "nearest" });
	}, [active, id]);

	const groups: { kind: MenuOption["kind"]; start: number; options: MenuOption[] }[] = [];
	options.forEach((option, index) => {
		const last = groups.at(-1);
		if (last?.kind === option.kind) last.options.push(option);
		else groups.push({ kind: option.kind, start: index, options: [option] });
	});
	const empty =
		kind === "path"
			? translate("No {{pathFinds}} match", { pathFinds: pathFinds(finds, "disjunction") })
			: EMPTY[kind];
	const status =
		options.length > 0 ? null : (notice ?? (loading ? translate("Searching") : empty));
	const label =
		kind === "path" ? sentence(pathFinds(finds, "conjunction")) : translate(LIST_LABEL[kind]);

	return (
		<div
			ref={list}
			id={id}
			role="listbox"
			aria-label={label}
			className={cn(
				"absolute right-0 left-0 z-20 max-h-64 overflow-y-auto rounded-lg border border-input bg-popover p-1 shadow-lg",
				below ? "top-full mt-1.5" : "bottom-full mb-1.5"
			)}
			onMouseDown={(event) => event.preventDefault()}
		>
			{status !== null && (
				<div role="status" className="px-2 py-1.5 text-[12px] text-muted-foreground">
					{status}
				</div>
			)}
			{groups.map((group) => (
				<div key={group.kind} role="group" aria-labelledby={`${id}-${group.kind}`}>
					<div
						id={`${id}-${group.kind}`}
						className="px-2 pt-1.5 pb-1 text-[11px] font-medium text-muted-foreground"
					>
						{translate(GROUPS[group.kind])}
					</div>
					{group.options.map((option, offset) => {
						const index = group.start + offset;
						const look = optionLook(option);
						return (
							<div
								key={index}
								id={optionId(id, index)}
								role="option"
								aria-selected={index === active}
								className={cn(ROW, index === active && "bg-accent")}
								onMouseMove={() => {
									if (index !== active) onActive(index);
								}}
								onClick={() => onPick(option)}
							>
								{look.icon}
								<span className="min-w-0 flex-1 truncate">{look.label}</span>
								{look.detail && (
									<span className="max-w-[55%] shrink-0 truncate text-[11.5px] text-muted-foreground">
										{look.detail}
									</span>
								)}
							</div>
						);
					})}
				</div>
			))}
		</div>
	);
};
