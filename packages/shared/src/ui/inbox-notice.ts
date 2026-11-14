// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The inbox changes you make from the sidebar and the notice they leave:
// "Settled 3 agents" with an Undo for five seconds, or what went wrong.
// Auto-settles that arrive live leave a notice too, whose Undo brings the
// agents back. One of these lives with each window's services.

import { t } from "../i18n";

import { createStore, type StoreApi } from "zustand/vanilla";

import type { Agent, AgentId, Timestamp } from "../bindings";
import type { ActionResult, DataSource, Unsubscribe } from "../data/source";
import {
	autoSettledText,
	doneText,
	failureLines,
	type InboxStep,
	inboxFailureOf,
	newlyAutoSettled,
	restoreSteps,
} from "../view/inbox-actions";
import type { ViewStore } from "../view/store";

/** How long a notice offers its Undo. */
export const UNDO_MS = 5000;

// How long after its answer a change still counts as yours, so the view
// changes it brings aren't taken for auto-settle's.
const SETTLE_DOWN_MS = 2000;

/** The notice after a change. */
export type InboxNotice = {
	/** Tells one notice from the next, for a key. */
	readonly id: number;
	/** What happened, such as "Settled 3 agents". */
	readonly text: string;
	/** The calls that undo it. */
	readonly undo: readonly InboxStep[];
};

/** What the sidebar shows under its list. */
export type InboxNoticeState = {
	/** The last change's notice, while its Undo lasts. */
	readonly notice: InboxNotice | null;
	/** What went wrong, one sentence for each reason, until dismissed. */
	readonly failures: readonly string[];
};

/** The notice store and what it needs to make changes. */
export type InboxNotices = {
	readonly store: StoreApi<InboxNoticeState>;
	readonly source: DataSource;
	readonly view: ViewStore;
	/** The time the notice's words describe. */
	readonly now: () => Timestamp;
	// Agents with a change of yours in flight, and how many.
	readonly ours: Map<AgentId, number>;
	timer?: ReturnType<typeof setTimeout>;
	nextId: number;
};

/** A store with no notice, for one window. */
export const createInboxNotices = (
	source: DataSource,
	view: ViewStore,
	now: () => Timestamp = Date.now
): InboxNotices => ({
	store: createStore<InboxNoticeState>(() => ({ notice: null, failures: [] })),
	source,
	view,
	now,
	ours: new Map(),
	nextId: 1,
});

const show = (notices: InboxNotices, text: string, undo: readonly InboxStep[]) => {
	clearTimeout(notices.timer);
	const id = notices.nextId++;
	notices.store.setState({ notice: { id, text, undo } });
	notices.timer = setTimeout(() => {
		if (notices.store.getState().notice?.id === id) notices.store.setState({ notice: null });
	}, UNDO_MS);
};

const hold = (notices: InboxNotices, agents: readonly AgentId[]) => {
	for (const id of agents) notices.ours.set(id, (notices.ours.get(id) ?? 0) + 1);
};

const release = (notices: InboxNotices, agents: readonly AgentId[]) => {
	setTimeout(() => {
		for (const id of agents) {
			const left = (notices.ours.get(id) ?? 1) - 1;
			if (left > 0) notices.ours.set(id, left);
			else notices.ours.delete(id);
		}
	}, SETTLE_DOWN_MS);
};

/** How a change ended for each agent: the ones it went through for, and the rest. */
export type InboxOutcome = {
	/** The agents it went through for, as they were before it. */
	readonly done: readonly Agent[];
	/** What went wrong, one sentence for each reason. */
	readonly failures: readonly string[];
};

/** Makes the calls, one after another, and sorts each agent's answer. */
const run = async (notices: InboxNotices, steps: readonly InboxStep[]): Promise<InboxOutcome> => {
	const done: Agent[] = [];
	const failures: string[] = [];
	for (const step of steps) {
		if (step.agents.length === 0) continue;
		const { view } = notices.view.getState();
		const before = step.agents.map((id) => view.tables.agent.get(id));
		hold(notices, step.agents);
		let results: readonly ActionResult[];
		try {
			results = await notices.source.setInbox([...step.agents], step.change);
		} catch (error) {
			const reason = error instanceof Error ? error.message : String(error);
			results = step.agents.map(
				() => ({ status: "error", error: { kind: "failed", message: reason } }) as const
			);
		} finally {
			release(notices, step.agents);
		}
		const failed: { agent: Agent; reason: string }[] = [];
		before.forEach((agent, i) => {
			if (!agent) return;
			const result = results[i];
			const reason = result ? inboxFailureOf(result) : t("No answer came back");
			if (reason === null) done.push(agent);
			else failed.push({ agent, reason });
		});
		failures.push(...failureLines(step.change, failed));
	}
	return { done, failures };
};

/**
 * Makes a change and leaves its notice: what it did with an Undo that puts
 * the agents back as they were, and what went wrong for any it didn't do.
 * `text` words the notice when the change alone can't, such as a snooze to
 * each account's own reset.
 */
export const changeInbox = async (
	notices: InboxNotices,
	steps: readonly InboxStep[],
	text?: (done: readonly Agent[]) => string
): Promise<InboxOutcome> => {
	const outcome = await run(notices, steps);
	const first = steps[0];
	const words =
		outcome.done.length === 0 || !first
			? null
			: text
				? text(outcome.done)
				: doneText(first.change, outcome.done, notices.now());
	if (words !== null) show(notices, words, restoreSteps(outcome.done));
	notices.store.setState({ failures: outcome.failures });
	return outcome;
};

/** Undoes the change the notice is for, while its Undo lasts. Whether there was one. */
export const undoNotice = (notices: InboxNotices): boolean => {
	const notice = notices.store.getState().notice;
	if (!notice) return false;
	clearTimeout(notices.timer);
	notices.store.setState({ notice: null });
	void run(notices, notice.undo).then(({ failures }) => notices.store.setState({ failures }));
	return true;
};

/** Hides the notice before its time. */
export const dismissNotice = (notices: InboxNotices): void => {
	clearTimeout(notices.timer);
	notices.store.setState({ notice: null });
};

/** Shows what went wrong with something that never reached a device. */
export const showFailure = (notices: InboxNotices, text: string): void =>
	notices.store.setState({ failures: [text] });

/** Hides what went wrong. */
export const dismissFailures = (notices: InboxNotices): void =>
	notices.store.setState({ failures: [] });

/**
 * Leaves a notice whenever auto-settle settles agents while the window
 * watches, with an Undo that unsettles them. Returns a function that stops.
 */
export const watchAutoSettles = (notices: InboxNotices): Unsubscribe =>
	notices.view.subscribe((next, previous) => {
		const settled = newlyAutoSettled(previous.view, next.view).filter(
			(agent) => !notices.ours.has(agent.id)
		);
		if (settled.length === 0) return;
		show(notices, autoSettledText(settled.length), [
			{ agents: settled.map((agent) => agent.id), change: { change: "unsettle" } },
		]);
	});
