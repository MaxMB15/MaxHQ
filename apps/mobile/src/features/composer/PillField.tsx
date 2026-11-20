// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A phone text field that holds pills. Each pill shows as its token in the
// pill's shade and acts as one piece, as text-field.ts tells, and so does
// a `#123` that shows as a pull request or issue. The word at the caret
// opens the menu the desktop's box opens for it, above the keyboard, and a
// pick puts the pill in its place. The composer, the answer fields and the
// dispatcher's box each say which words open a menu, whose folder and repo
// it searches, which agents and notes `@` names, and what a picked file
// becomes, as the desktop's pill-menu.tsx takes them.

import type { Agent, AgentId, DeviceId, ProjectId } from "@maxhq/shared/bindings";
import { menuPick, type MenuOption, menuWord, useMenuContents } from "@maxhq/shared/composer/menu";
import {
	type AgentRef,
	type FileFacts,
	gitKey,
	gitNumbers,
	markedText,
	type Segment,
} from "@maxhq/shared/pills/draft";
import { type GitPillsStore, shownItem } from "@maxhq/shared/pills/git-pills";
import {
	editField,
	type FieldEdit,
	fieldOf,
	type FieldSelection,
	fieldSpans,
	fieldTrigger,
	markedOf,
	replaceMarked,
	selectionBefore,
	shownOf,
	tokensOf,
} from "@maxhq/shared/pills/text-field";
import type { TriggerKind } from "@maxhq/shared/pills/trigger";
import { useSkillFeed } from "@maxhq/shared/skills/feed";
import { noteChoices } from "@maxhq/shared/view/notes";
import type { ViewState } from "@maxhq/shared/view/replica";
import {
	type ReactNode,
	type RefObject,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import {
	Platform,
	StyleSheet,
	Text,
	type TextInputInstance,
	type TextInputSelectionChangeEvent,
} from "react-native";
import { useStore } from "zustand";

import { useServices, useView } from "../../services";
import { color, tint } from "../../theme";
import { SuggestionList } from "./SuggestionList";

/** The notes a field's `@` menu finds: all, with `own`'s first, or only `own`'s. */
export type MenuNotes = { readonly own: ProjectId | null; readonly only: boolean };

export type PillFieldSetup = {
	/** The TextInput the caller draws the field in, to place the caret. */
	readonly ref: RefObject<TextInputInstance | null>;
	/** The field's draft. */
	readonly segments: readonly Segment[];
	readonly onChange: (segments: Segment[]) => void;
	/** A file pill's name and kind, by key, for its token. */
	readonly factsOf: (key: string) => FileFacts | undefined;
	readonly gitPills: GitPillsStore;
	/** The agent whose folder and repo the menu searches, and whom `@` leaves out, or null. */
	readonly agent: AgentId | null;
	/** The words that open a menu. */
	readonly kinds: readonly TriggerKind[];
	/** What a picked file or folder puts in, or null when `@` finds none. */
	readonly onPath: ((path: string) => readonly Segment[]) | null;
	/** Set when `@` names only the agents on this device. */
	readonly agentsOn: DeviceId | null;
	/** The notes `@` finds, or null for none. */
	readonly notes: MenuNotes | null;
	/** An agent as the pill `@` puts in, or null for none. */
	readonly agentPill: (agent: Agent) => AgentRef | null;
};

export type PillField = {
	/** What the field's TextInput takes, besides its ref, in place of a value. */
	readonly input: {
		readonly children: ReactNode;
		readonly onChangeText: (text: string) => void;
		readonly onSelectionChange: (event: TextInputSelectionChangeEvent) => void;
	};
	/** The open menu, to draw above the field, or null. */
	readonly menu: ReactNode;
	/** Puts segments in at the caret, or at the end before the field had one. */
	readonly insert: (segments: readonly Segment[]) => void;
	/** Keeps the word at the end of `segments` shut, as after the draft was set anew. */
	readonly shutAt: (segments: readonly Segment[]) => void;
};

/**
 * The selections before the latest, newest last. iOS tells the new
 * selection before the new text, and Android after, so the one the edit
 * started from is the second to last on iOS and the last on Android.
 */
type Selections = { readonly prev: FieldSelection | null; readonly last: FieldSelection | null };

/** The draft and caret a field's events start from. */
type Latest = { readonly segments: readonly Segment[]; readonly caret: number | null };

const NOTHING_YET: Latest = { segments: [], caret: null };

const candidates = ({ prev, last }: Selections) =>
	Platform.OS === "ios" ? [prev, last] : [last, prev];

/** A field's draft drawn with its pills, and the menu for the word at its caret. */
export const usePillField = (setup: PillFieldSetup): PillField => {
	const {
		ref,
		segments,
		onChange,
		factsOf,
		gitPills,
		agent,
		kinds,
		onPath,
		agentsOn,
		agentPill,
	} = setup;
	const { source } = useServices();
	const view = useView((state) => state.view);
	const gitState = useStore(gitPills);
	const tokenOf = tokensOf(segments, factsOf);
	const field = fieldOf(segments, tokenOf);
	const marked = markedText(segments);
	/** The caret in the shown text, or null while a range is selected or before the field had one. */
	const [caret, setCaret] = useState<number | null>(null);
	/** Where the caret goes once the field shows the draft anew. */
	const [placeAt, setPlaceAt] = useState<{ readonly at: number } | null>(null);
	/** The word whose menu a pick or a new draft shut, which stays shut until the word changes. */
	const [dismissed, setDismissed] = useState(() => menuWord(marked, marked.length));
	const selections = useRef<Selections>({ prev: null, last: null });
	// The draft and caret as the last event left them, for an event that
	// comes before the render that shows them. The layout effect fills it
	// in before the field can take a touch.
	const latest = useRef<Latest>(NOTHING_YET);
	useLayoutEffect(() => {
		latest.current = { segments, caret };
	}, [segments, caret]);

	useEffect(() => {
		if (placeAt !== null) ref.current?.setSelection(placeAt.at, placeAt.at);
	}, [placeAt, ref]);

	const moveCaret = (at: number) => {
		const collapsed = { start: at, end: at };
		selections.current = { prev: collapsed, last: collapsed };
		latest.current = { ...latest.current, caret: at };
		setCaret(at);
		setPlaceAt({ at });
	};

	const changed = (next: Segment[]) => {
		latest.current = { ...latest.current, segments: next };
		onChange(next);
		if (agent !== null) gitPills.getState().want(gitNumbers(markedText(next)));
	};

	/** Puts an edit's draft in the field, with the caret where it says. */
	const place = (edit: FieldEdit) => {
		changed(edit.segments);
		moveCaret(shownOf(fieldOf(edit.segments, tokensOf(edit.segments, factsOf)), edit.caret));
	};

	const onChangeText = (next: string) => {
		const current = latest.current.segments;
		const tokens = tokensOf(current, factsOf);
		const before = selectionBefore(
			fieldOf(current, tokens).text,
			next,
			candidates(selections.current)
		);
		const change = editField(current, tokens, next, before);
		changed(change.segments);
		if (change.same) {
			const collapsed = { start: change.caret, end: change.caret };
			selections.current = { prev: collapsed, last: collapsed };
			latest.current = { ...latest.current, caret: change.caret };
			setCaret(change.caret);
		} else moveCaret(change.caret);
	};

	const onSelectionChange = (event: TextInputSelectionChangeEvent) => {
		const { start, end } = event.nativeEvent.selection;
		selections.current = { prev: selections.current.last, last: { start, end } };
		const at = start === end ? end : null;
		latest.current = { ...latest.current, caret: at };
		setCaret(at);
	};

	const agents = view.tables.agent;
	const workspaces = view.tables.workspace;
	const others = useMemo(
		() =>
			[...agents.values()].filter(
				(other) => agentsOn === null || workspaces.get(other.workspace)?.device === agentsOn
			),
		[agents, agentsOn, workspaces]
	);
	// The draft is read for the field before the menu, so the menu's word stays as it is.
	const spans = fieldSpans(segments, tokenOf, (key) => shownItem(gitState, key) !== null);
	const found = caret === null ? null : fieldTrigger(segments, tokenOf, caret);
	const trigger = found && kinds.includes(found.kind) ? found : null;
	const word = trigger && menuWord(marked, caret === null ? null : markedOf(field, caret));
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
	const notes = useNoteChoices(view, noteQuery, own, only);
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

	const pick = (option: MenuOption) => {
		if (!menu) return;
		const chosen = menuPick(option, { onPath, agentPill });
		if (chosen.seed)
			gitPills
				.getState()
				.seed(new Map([[gitKey({ repo: null, number: chosen.seed.number }), chosen.seed]]));
		if (chosen.keep !== null) {
			gitPills.getState().keep(gitKey({ repo: null, number: chosen.keep }));
			setDismissed(word);
		}
		if (chosen.segments)
			place(
				replaceMarked(
					latest.current.segments,
					menu.rangeStart,
					menu.rangeEnd,
					chosen.segments,
					chosen.spaced
				)
			);
	};

	const insert = (inserted: readonly Segment[]) => {
		if (inserted.length === 0) return;
		const { segments: current, caret: at } = latest.current;
		const shown = fieldOf(current, tokensOf(current, factsOf));
		const offset = at === null ? markedText(current).length : markedOf(shown, at, true);
		place(replaceMarked(current, offset, offset, inserted));
	};

	return {
		input: {
			// One Text holds every run, so an empty draft still empties the field.
			children: (
				<Text>
					{spans.map((span, index) =>
						span.kind === "text" ? (
							span.text
						) : (
							<Text
								key={index}
								style={
									span.kind === "pill" && span.pill.type === "visual"
										? styles.visual
										: styles.pill
								}
							>
								{span.text}
							</Text>
						)
					)}
				</Text>
			),
			onChangeText,
			onSelectionChange,
		},
		menu: menu && (
			<SuggestionList
				kind={menu.kind}
				contents={contents}
				finds={{ files: onPath !== null, notes: setup.notes !== null }}
				onPick={pick}
			/>
		),
		insert,
		shutAt: (next) => {
			const text = markedText(next);
			setDismissed(menuWord(text, text.length));
		},
	};
};

/**
 * The notes `@` finds for `query`, or none when null. Its own hook, so the
 * memo keeps apart from the trigger the query comes from.
 */
const useNoteChoices = (
	view: ViewState,
	query: string | null,
	own: ProjectId | null,
	only: boolean
) =>
	useMemo(
		() =>
			query === null
				? []
				: noteChoices(view, own, query).filter((choice) => !only || choice.own),
		[query, only, own, view]
	);

const styles = StyleSheet.create({
	pill: { color: color.primary, backgroundColor: tint(color.primary, 0.16) },
	visual: { color: color.visual, backgroundColor: tint(color.visual, 0.16) },
});
