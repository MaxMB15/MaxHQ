// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A pill field for what you type outside a message, such as a request to
// the dispatcher: text with a mark where each pill sits, and the pills. It
// takes no files and no pull requests, as the desktop's answer field takes
// none where no agent's repo is near: `@` names the agents in the place, and
// a note only when the place has a project.

import { type RefObject, useMemo, useState } from "react";
import type { TextInputInstance } from "react-native";

import type { Agent, DeviceId } from "@maxhq/shared/bindings";
import { agentPillOf } from "@maxhq/shared/pills/agent-pill";
import { type AnswerPlace, draftOf, type Typed, typedOf } from "@maxhq/shared/pills/answer";
import type { AgentRef, Segment } from "@maxhq/shared/pills/draft";
import { createGitPills } from "@maxhq/shared/pills/git-pills";
import type { TriggerKind } from "@maxhq/shared/pills/trigger";

import { useServices } from "../../services";
import { type PillField, usePillField } from "./PillField";

const PATHS: readonly TriggerKind[] = ["path"];

const NO_FACTS = () => undefined;

const NOTHING_FOUND = async () => null;

export type TypedFieldSetup = {
	readonly ref: RefObject<TextInputInstance | null>;
	/** What you typed, with a mark where each pill sits. */
	readonly text: string;
	/** The pills in it. */
	readonly refs: Typed["refs"];
	readonly onChange: (typed: Typed) => void;
	/** Where what you type goes, which says which agents and notes `@` names. */
	readonly place: AnswerPlace;
	/** The device an agent pill names when the place has none, such as the dispatcher's. */
	readonly device: DeviceId | null;
};

/** A field for what you typed, with the menu `@` opens. */
export const useTypedField = ({
	ref,
	text,
	refs,
	onChange,
	place,
	device,
}: TypedFieldSetup): PillField => {
	const services = useServices();
	const segments = useMemo(() => draftOf({ text, refs }).segments, [text, refs]);
	// No `#` opens a menu here, so nothing is ever looked up.
	const [gitPills] = useState(() => createGitPills(NOTHING_FOUND));
	return usePillField({
		ref,
		segments,
		onChange: (next: Segment[]) => onChange(typedOf({ segments: next, gitItems: [] })),
		factsOf: NO_FACTS,
		gitPills,
		agent: null,
		kinds: PATHS,
		onPath: null,
		agentsOn: place.device,
		notes: place.project === null ? null : { own: place.project, only: true },
		agentPill: (other: Agent): AgentRef | null => {
			const view = services.view.getState().view;
			const fallback = place.device ?? device;
			if (fallback === null && !view.tables.workspace.has(other.workspace)) return null;
			return agentPillOf(view, other, fallback ?? ("" as DeviceId));
		},
	});
};
