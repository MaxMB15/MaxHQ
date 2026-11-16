// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The menu a text box opens for the word at the caret, and what a pick puts
// in its place. The composer and the answer fields share it: each says
// which words open a menu, whose folder and repo it searches, which agents
// and notes `@` names, and what a picked file becomes. Escape shuts the
// menu for its word, which stays shut until the word changes. What each
// menu offers, and what a pick puts in, are shared with the phone.

import type { Agent, AgentId, DeviceId, ProjectId } from "@maxhq/shared/bindings";
import { menuPick, menuWord } from "@maxhq/shared/composer/menu";
import { type AgentRef, gitKey, type Segment } from "@maxhq/shared/pills/draft";
import { type GitPillsStore, shownItem } from "@maxhq/shared/pills/git-pills";
import { detectTrigger, type TriggerKind } from "@maxhq/shared/pills/trigger";
import { useSkillFeed } from "@maxhq/shared/skills/feed";
import { noteChoices } from "@maxhq/shared/view/notes";
import { type ReactNode, type RefObject, useId, useMemo, useState } from "react";
import { useStore } from "zustand";

import { useServices, useView } from "@/app/services";

import type { MenuKey, PromptEditorHandle } from "./editor";
import { ComposerMenu, type MenuOption, optionId, useMenuContents } from "./menu";

/** The notes a box's `@` menu finds: all, with `own`'s first, or only `own`'s. */
export type MenuNotes = { readonly own: ProjectId | null; readonly only: boolean };

export type PillMenuSetup = {
	readonly editor: RefObject<PromptEditorHandle | null>;
	/** The box's text and the caret in it, as its last change left them. */
	readonly text: string;
	readonly caret: number | null;
	/** The agent whose folder and repo the menu searches, and whom `@` leaves out, or null. */
	readonly agent: AgentId | null;
	/** The words that open a menu. */
	readonly kinds: readonly TriggerKind[];
	/** The text whose last word starts shut, when not `text`, such as a draft the box opens with. */
	readonly shut?: string;
	/** What a picked file or folder puts in, or null when `@` finds none. */
	readonly onPath: ((path: string) => readonly Segment[]) | null;
	/** Set when `@` names only the agents on this device. */
	readonly agentsOn: DeviceId | null;
	/** The notes `@` finds, or null for none. */
	readonly notes: MenuNotes | null;
	readonly gitPills: GitPillsStore;
	/** An agent as the pill `@` puts in, or null for none. */
	readonly agentPill: (agent: Agent) => AgentRef | null;
	/** Whether the menu opens below the box. */
	readonly below?: boolean;
};

export type PillMenu = {
	/** The open menu, to draw in the box, or null. */
	readonly element: ReactNode;
	/** The ARIA attributes that tie the box to the open menu. */
	readonly attributes: Record<string, string>;
	/** A key from the box. Returns whether the menu took it. */
	readonly onKey: (key: MenuKey) => boolean;
	/** Shuts the open menu until its word changes, as a click away does. */
	readonly close: () => void;
	/** Keeps a word at the end of `text` shut, as after the box's text was set. */
	readonly shutAt: (text: string) => void;
};

/** The menu for the word at the caret, unless Escape shut it. */
export const usePillMenu = (setup: PillMenuSetup): PillMenu => {
	const { editor, text, caret, agent, kinds, onPath, agentsOn, gitPills, agentPill } = setup;
	const { source } = useServices();
	const view = useView((state) => state.view);
	const gitState = useStore(gitPills);
	const id = useId();
	/**
	 * The word whose menu Escape closed, which stays closed until the word
	 * changes. A menu opens as you type, so a kept draft opens with it shut.
	 */
	const [dismissed, setDismissed] = useState(() => {
		const opened = setup.shut ?? text;
		return menuWord(opened, opened.length);
	});
	const [active, setActive] = useState<{ menu: string; index: number } | null>(null);

	const agents = view.tables.agent;
	const workspaces = view.tables.workspace;
	const others = useMemo(
		() =>
			[...agents.values()].filter(
				(other) => agentsOn === null || workspaces.get(other.workspace)?.device === agentsOn
			),
		[agents, agentsOn, workspaces]
	);
	const found = caret === null ? null : detectTrigger(text, caret);
	const trigger = found && kinds.includes(found.kind) ? found : null;
	const word = trigger && menuWord(text, caret);
	const menu = word !== null && word !== dismissed ? trigger : null;
	const typed = menu?.kind === "git-item" && /^\d+$/.test(menu.query) ? Number(menu.query) : null;
	const keep =
		typed !== null && shownItem(gitState, gitKey({ repo: null, number: typed })) ? typed : null;
	const skills = useSkillFeed(
		source,
		view,
		agent ?? "",
		agent !== null && menu?.kind === "skill"
	);
	const noteQuery = menu?.kind === "path" && setup.notes !== null ? menu.query : null;
	const own = setup.notes?.own ?? null;
	const only = setup.notes?.only ?? false;
	const notes = useMemo(
		() =>
			noteQuery === null
				? []
				: noteChoices(view, own, noteQuery).filter((choice) => !only || choice.own),
		[noteQuery, only, own, view]
	);
	const contents = useMenuContents({
		source,
		agent,
		agents: others,
		notes,
		files: onPath !== null,
		trigger: menu,
		keep,
		skills,
	});
	const menuKey = `${word}:${contents.options.length}`;
	const activeIndex = active?.menu === menuKey ? active.index : 0;

	const pick = (option: MenuOption) => {
		if (!menu || !editor.current) return;
		const { rangeStart: from, rangeEnd: to } = menu;
		const chosen = menuPick(option, { onPath, agentPill });
		if (chosen.seed)
			gitPills
				.getState()
				.seed(new Map([[gitKey({ repo: null, number: chosen.seed.number }), chosen.seed]]));
		if (chosen.keep !== null) {
			gitPills.getState().keep(gitKey({ repo: null, number: chosen.keep }));
			setDismissed(word);
		}
		if (chosen.segments) editor.current.replaceRange(from, to, chosen.segments, chosen.spaced);
	};

	const onKey = (key: MenuKey): boolean => {
		if (!menu) return false;
		const { options } = contents;
		switch (key) {
			case "close":
				setDismissed(word);
				return true;
			case "up":
			case "down": {
				if (options.length === 0) return false;
				const step = key === "up" ? -1 : 1;
				const index = (activeIndex + step + options.length) % options.length;
				setActive({ menu: menuKey, index });
				return true;
			}
			case "pick": {
				const option = options[activeIndex];
				if (!option) return false;
				pick(option);
				return true;
			}
		}
	};

	const attributes: Record<string, string> = menu
		? {
				"aria-controls": id,
				"aria-autocomplete": "list",
				...(contents.options.length > 0
					? { "aria-activedescendant": optionId(id, activeIndex) }
					: {}),
			}
		: {};

	return {
		element: menu && (
			<ComposerMenu
				id={id}
				kind={menu.kind}
				contents={contents}
				active={activeIndex}
				onActive={(index) => setActive({ menu: menuKey, index })}
				onPick={pick}
				below={setup.below}
				finds={{ files: onPath !== null, notes: setup.notes !== null }}
			/>
		),
		attributes,
		onKey,
		close: () => setDismissed(word),
		shutAt: (now) => setDismissed(menuWord(now, now.length)),
	};
};
