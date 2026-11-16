// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A command's output in the composer. Its pill follows the command, from
// its terminal while the view holds it and from its timeline item after
// that, and its preview shows the last lines as the message would take
// them. When the message goes, each output pill takes those lines.

import { useCallback, useMemo, useSyncExternalStore } from "react";

import type { TerminalId, Timestamp } from "@maxhq/shared/bindings";
import type { OutputRef, Segment } from "@maxhq/shared/pills/draft";
import { fillOutput, OUTPUT_LINES, userRunOf } from "@maxhq/shared/terminal/output";
import { terminalById } from "@maxhq/shared/terminal/select";
import type { TimelineStore } from "@maxhq/shared/timeline/store";

import { useServicesIfAny } from "@/app/services";
import { OutputPreview } from "@/pills/output-preview";
import { type PillPresentation, presentPill } from "@/pills/registry";
import { useTerminalActions } from "@/terminal";

const NO_STORE = () => () => {};

/** The command's item in the timeline, while it holds one. */
const useRunItem = (timeline: TimelineStore | undefined, terminal: TerminalId | null) =>
	useSyncExternalStore(timeline?.subscribe ?? NO_STORE, () =>
		timeline && terminal ? userRunOf(timeline.getState(), terminal) : undefined
	);

/** The command's terminal, while the view holds it. */
const useTerminal = (terminal: TerminalId | null) => {
	const view = useServicesIfAny()?.view;
	return useSyncExternalStore(view?.subscribe ?? NO_STORE, () =>
		view && terminal ? terminalById(view.getState().view, terminal) : undefined
	);
};

/** Which terminal a pill follows: none once it holds lines picked in the drawer. */
const followed = (ref: OutputRef): TerminalId | null => (ref.lines ? null : (ref.terminal ?? null));

/** The preview of a pill in the composer, read when it opens. */
const ComposerPreview = ({
	output,
	terminal,
}: {
	output: OutputRef;
	terminal: TerminalId | null;
}) => {
	const actions = useTerminalActions();
	const live = output.running && terminal ? actions.text(terminal, OUTPUT_LINES) : null;
	return (
		<OutputPreview
			output={live ? { ...output, text: live.text, lines_total: live.total } : output}
			editable
		/>
	);
};

/** How an output pill in the composer looks now. */
export const useOutputPill = (
	ref: OutputRef,
	timeline: TimelineStore | undefined,
	now: Timestamp
): PillPresentation => {
	const id = followed(ref);
	const item = useRunItem(timeline, id);
	const terminal = useTerminal(id);
	const shown = useMemo(
		() => fillOutput(ref, { item, terminal, now: ref.ran_at }),
		[ref, item, terminal]
	);
	return presentPill(
		{
			kind: "output",
			ref: shown,
			preview: <ComposerPreview output={shown} terminal={id} />,
		},
		{ now }
	);
};

/**
 * Fills each output pill in a message with what goes to the agent: the
 * last 200 lines, from the whole output once the command ended and from
 * the live terminal while it runs.
 */
export const useOutputFill = (timeline: TimelineStore) => {
	const services = useServicesIfAny();
	const actions = useTerminalActions();
	return useCallback(
		(segments: readonly Segment[]): Segment[] =>
			segments.map((segment) => {
				if (segment.type !== "pill" || segment.pill.type !== "output") return segment;
				const id = followed(segment.pill);
				if (id === null) return segment;
				const view = services?.view.getState().view;
				const pill = fillOutput(segment.pill, {
					item: userRunOf(timeline.getState(), id),
					terminal: view ? terminalById(view, id) : undefined,
					live: actions.text(id, OUTPUT_LINES),
					now: Date.now(),
				});
				return { type: "pill", pill };
			}),
		[actions, services, timeline]
	);
};
