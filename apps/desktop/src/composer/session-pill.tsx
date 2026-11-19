// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The session's pill in the composer: one pill for the open card's
// commands, which goes in by itself once the card holds a command the
// agent may see, and leaves once a message closed the card or none is
// shared. One you removed stays out for that card, through a restart
// too. Its preview lists the commands with ticks, and unticking one keeps
// it from the agent. When the message goes, the pill takes the shared
// commands' last lines.

import { Check } from "lucide-react";
import { type RefObject, useCallback, useEffect, useSyncExternalStore } from "react";

import type { AgentId, ItemId, SessionBlock, Timestamp } from "@maxhq/shared/bindings";
import type { SavedDrafts } from "@maxhq/shared/composer/saved-drafts";
import { attempt, failureOf } from "@maxhq/shared/composer/send";
import type { DataSource } from "@maxhq/shared/data/source";
import { t, useI18n } from "@maxhq/shared/i18n";
import type { SessionRef, Segment } from "@maxhq/shared/pills/draft";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import {
	blockFailed,
	blockRunning,
	cardSummary,
	fillSession,
	openCardOf,
	sessionCardOf,
	sessionPill,
	sessionPillLook,
	type UserSessionItem,
	wantsPill,
} from "@maxhq/shared/terminal/session";
import type { TimelineStore } from "@maxhq/shared/timeline/store";

import { useServices, useServicesIfAny } from "@/app/services";
import { cn } from "@/lib/utils";
import { type PillPresentation, presentPill } from "@/pills/registry";
import { useTerminalActions } from "@/terminal";

import type { PromptEditorHandle } from "./editor";
import { isSessionOf, removePill, sessionItems, useHeldSessions } from "./held-pills";
import { draftsOf } from "./saved-drafts";

const NO_STORE = () => () => {};

/**
 * Notes that this card's pill went in the agent's composer, kept with the
 * saved draft, so once you removed it, it stays out after a restart too.
 */
export const markPlaced = (drafts: SavedDrafts, agent: AgentId, item: ItemId) => {
	const placed = drafts.placed(agent);
	if (!placed.has(item)) drafts.setPlaced(agent, new Set(placed).add(item));
};

/** Puts a card's pill back in the agent's composer after you removed it. */
export const addSessionPill = (
	drafts: SavedDrafts,
	agent: AgentId,
	card: Pick<UserSessionItem, "id">,
	deviceName: string
) => {
	markPlaced(drafts, agent, card.id);
	quoteInbox.send(agent, sessionPill(card, deviceName), true);
};

/** The card with this ID, while the timeline holds it. */
const useCard = (timeline: TimelineStore | undefined, item: ItemId | null | undefined) =>
	useSyncExternalStore(timeline?.subscribe ?? NO_STORE, () =>
		timeline && item ? sessionCardOf(timeline.getState(), item) : undefined
	);

/**
 * Keeps the open card's pill in the agent's composer: in once the card
 * holds a shared command, unless you removed it, and out once a message
 * closed the card or nothing in it is shared. A pill whose card the
 * loaded timeline doesn't hold goes too.
 */
export const useSessionPills = (
	agent: AgentId,
	timeline: TimelineStore,
	segments: readonly Segment[],
	editor: RefObject<PromptEditorHandle | null>,
	deviceName: string
) => {
	const drafts = draftsOf(useServices());
	useHeldSessions(agent, segments, editor);
	const open = useSyncExternalStore(timeline.subscribe, () => openCardOf(timeline.getState()));
	const ready = useSyncExternalStore(
		timeline.subscribe,
		() => timeline.getState().status === "ready"
	);
	useEffect(() => {
		const before = drafts.placed(agent);
		let placed = before;
		const holds = sessionItems(segments);
		for (const item of [...holds]) {
			const card = sessionCardOf(timeline.getState(), item);
			// A pill restored with the draft waits for its card while the timeline loads.
			if (card ? wantsPill(card) : !ready) continue;
			removePill(editor.current, isSessionOf(item));
			holds.delete(item);
			// Shared again, its pill comes back.
			if (card && !card.sealed && placed.has(item)) {
				const next = new Set(placed);
				next.delete(item);
				placed = next;
			}
		}
		if (open && wantsPill(open) && !holds.has(open.id) && !placed.has(open.id)) {
			placed = new Set(placed).add(open.id);
			const pill = sessionPill(open, deviceName);
			// The pill's view renders as it goes in, which React won't do from an effect.
			queueMicrotask(() => quoteInbox.send(agent, pill, false));
		}
		if (placed !== before) drafts.setPlaced(agent, placed);
	}, [agent, open, ready, segments, editor, timeline, deviceName, drafts]);
};

/**
 * Fills each session pill in a message with what goes to the agent: its
 * card's shared commands, each with its last lines, from the live
 * terminal while one runs. A second pill for the same card goes, and so
 * does one whose card this timeline doesn't hold.
 */
export const useSessionFill = (timeline: TimelineStore) => {
	const actions = useTerminalActions();
	return useCallback(
		(segments: readonly Segment[]): Segment[] => {
			const seen = new Set<ItemId>();
			return segments.flatMap((segment): Segment[] => {
				if (segment.type !== "pill" || segment.pill.type !== "session") return [segment];
				const { item: id } = segment.pill;
				if (!id) return [segment];
				if (seen.has(id)) return [];
				seen.add(id);
				const card = sessionCardOf(timeline.getState(), id);
				if (!card) return [];
				const pill = fillSession(segment.pill, card, {
					now: Date.now(),
					live: (block) => actions.runningText(card.terminal, block.start),
				});
				return [{ type: "pill", pill }];
			});
		},
		[actions, timeline]
	);
};

/** Keeps the caret in the text box when a tick in the preview is pressed. */
const keepFocus = (event: { preventDefault: () => void }) => event.preventDefault();

/** How a block in the preview ended, such as "exit 1", or that it runs. */
const blockState = (block: SessionBlock): string | null => {
	if (block.private) return t("private");
	if (blockRunning(block)) return t("running");
	return block.process.state === "exited" && block.process.code !== null
		? t("exit {{code}}", { code: block.process.code })
		: null;
};

/** Keeps a block from the agent, or shares it again. */
export const shareBlock = async (
	source: Pick<DataSource, "sessionShare">,
	agent: AgentId,
	card: Pick<UserSessionItem, "id">,
	block: Pick<SessionBlock, "id" | "private">
): Promise<string | null> => {
	const reason = failureOf(
		await attempt(() => source.sessionShare(agent, card.id, block.id, !block.private))
	);
	if (reason === null) return null;
	return reason
		? t("Couldn't change who sees the command. {{reason}}.", { reason })
		: t("Couldn't change who sees the command.");
};

/** The pill's preview: the card's commands, each with a tick for whether the agent sees it. */
export const SessionPreview = ({
	card,
	deviceName,
	agent,
}: {
	card: UserSessionItem;
	deviceName: string;
	agent: AgentId | undefined;
}) => {
	useI18n();
	const services = useServicesIfAny();
	const { total, shared } = cardSummary(card);
	const acts = agent !== undefined && services !== null && !card.sealed;
	return (
		<div className="w-[300px]" data-session-preview="">
			<div className="mb-1.5">
				<b className="text-[12.5px]">
					{t("Shell session on {{device}}", { device: deviceName })}
				</b>{" "}
				<span className="text-[11.5px] text-subtle-foreground">
					{t("· {{shared}} of {{total}} go to the agent", { shared, total })}
				</span>
			</div>
			<ul className="flex flex-col gap-0.5">
				{card.blocks.map((block) => {
					const state = blockState(block);
					return (
						<li key={block.id} className="flex min-w-0 items-center gap-2 py-0.5">
							<button
								type="button"
								role="checkbox"
								aria-checked={!block.private}
								aria-label={t("Send {{command}} to the agent", {
									command: block.command,
								})}
								disabled={!acts}
								onMouseDown={keepFocus}
								onClick={() => {
									if (acts && services)
										void shareBlock(services.source, agent, card, block);
								}}
								className={cn(
									"grid size-3.5 shrink-0 cursor-pointer place-items-center rounded-[3px] border border-input outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default",
									!block.private &&
										"border-primary bg-primary text-primary-foreground"
								)}
							>
								{block.private ? null : (
									<Check aria-hidden className="size-2.5" strokeWidth={3} />
								)}
							</button>
							<span
								className={cn(
									"min-w-0 truncate font-mono text-[11.5px]",
									block.private && "text-muted-foreground"
								)}
							>
								{block.command}
							</span>
							{state === null ? null : (
								<span
									className={cn(
										"ml-auto shrink-0 text-[11px]",
										block.private
											? "text-subtle-foreground"
											: blockFailed(block)
												? "text-status-error-text"
												: blockRunning(block)
													? "text-status-busy-text"
													: "text-status-ready-text"
									)}
								>
									{state}
								</span>
							)}
						</li>
					);
				})}
			</ul>
		</div>
	);
};

/** How a session pill in the composer looks now, from its card. */
export const useSessionPill = (
	ref: SessionRef,
	timeline: TimelineStore | undefined,
	agent: AgentId | undefined,
	now: Timestamp
): PillPresentation => {
	const card = useCard(timeline, ref.item);
	return presentPill(
		{
			kind: "session",
			ref,
			look: card ? sessionPillLook(card) : null,
			preview: card ? (
				<SessionPreview card={card} deviceName={ref.device_name} agent={agent} />
			) : null,
		},
		{ now }
	);
};
