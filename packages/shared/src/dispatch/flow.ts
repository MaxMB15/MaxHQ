// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Asking the dispatcher, the same on every client: what you've typed, the
// call on its way, a note when something didn't go through, and how the last
// plan you confirmed went. Asking, confirming and dismissing go through
// here; each client keeps one of these and draws it its own way.

import { createStore, type StoreApi } from "zustand/vanilla";

import type { AccountId, Agent, DeviceId, Dispatcher, Plan, PlanOutcome } from "../bindings";
import { failureOf } from "../data/failure";
import type { ActionResult, DataSource } from "../data/source";
import { typedFor } from "../pills/answer";
import type { InlineRef } from "../pills/wire";
import { sentence } from "../status/format";
import type { ViewState } from "../view/replica";

import { isWorking } from "./conversation";

/** A plan you confirmed, and how each of its items went. */
export type PlanRun = { readonly plan: Plan; readonly results: readonly PlanOutcome[] };

/** Where asking the dispatcher stands. */
export type DispatchState = {
	/** What you've typed and not sent yet, with a mark where each pill sits. */
	readonly text: string;
	/** The pills in what you've typed. */
	readonly refs: readonly InlineRef[];
	/** Set once you ask for a new conversation, until your next request starts it. */
	readonly fresh: boolean;
	/** Set while a request, a confirm or a dismiss is on its way. */
	readonly busy: boolean;
	/** What went wrong last, as a sentence. */
	readonly note: string | null;
	/** The plan you confirmed last, until you ask again or start over. */
	readonly ran: PlanRun | null;
};

/** The state and the calls it makes. */
export type DispatchFlow = {
	readonly store: StoreApi<DispatchState>;
	readonly source: Pick<DataSource, "askDispatcher" | "confirmPlan" | "dismissPlan">;
};

/** Nothing typed, asked or confirmed yet. */
export const createDispatchFlow = (source: DispatchFlow["source"]): DispatchFlow => ({
	store: createStore<DispatchState>(() => ({
		text: "",
		refs: [],
		fresh: false,
		busy: false,
		note: null,
		ran: null,
	})),
	source,
});

/** Keeps what you typed, and the pills in it. */
export const setDraft = (flow: DispatchFlow, text: string, refs: readonly InlineRef[] = []): void =>
	flow.store.setState({ text, refs });

/** The note when there's no Mac account the dispatcher can run as. */
export const SIGN_IN_NOTE = "Sign in to Claude or Codex on this Mac to use the dispatcher.";

/** Where a request goes: the dispatcher's device, and the account it runs as there. */
export type DispatchTarget = {
	readonly device: DeviceId | null;
	readonly account: AccountId | null;
	/** Set when the dispatcher's maxd takes pills; an older one gets them as words. */
	readonly pills?: boolean;
};

/** A failure as the note under the box. */
export const noteOf = (failure: string): string => `${sentence(failure)}.`;

/** What a call came back with, or why it threw. */
export const tryCall = async (run: () => Promise<ActionResult>): Promise<ActionResult | string> => {
	try {
		return await run();
	} catch (error) {
		return error instanceof Error ? error.message : "The dispatcher didn't answer";
	}
};

/** Why a call failed, if it did. */
export const failureIn = (result: ActionResult | string): string | null =>
	typeof result === "string" ? result : failureOf(result);

/**
 * Sends what you typed, in a new conversation once you started over.
 * Nothing goes while the box is blank or another call is on its way. The
 * box clears once the request is in. Returns whether it went.
 */
export const ask = async (flow: DispatchFlow, target: DispatchTarget): Promise<boolean> => {
	const { text, refs, fresh, busy } = flow.store.getState();
	const asked = typedFor({ text: text.trim(), refs }, target.pills ?? false);
	if (!asked.text || busy) return false;
	const { device, account } = target;
	if (!device || !account) {
		flow.store.setState({ note: SIGN_IN_NOTE });
		return false;
	}
	flow.store.setState({ busy: true, note: null, ran: null });
	const failure = failureIn(
		await tryCall(() =>
			flow.source.askDispatcher(device, asked.text, account, fresh, asked.refs)
		)
	);
	flow.store.setState(
		failure
			? { busy: false, note: noteOf(failure) }
			: { busy: false, text: "", refs: [], fresh: false }
	);
	return failure === null;
};

/** Runs the plan shown, and keeps how each item went. Returns whether it ran. */
export const confirm = async (
	flow: DispatchFlow,
	device: DeviceId | null,
	plan: Plan | null
): Promise<boolean> => {
	if (!device || !plan || flow.store.getState().busy) return false;
	flow.store.setState({ busy: true, note: null });
	const result = await tryCall(() => flow.source.confirmPlan(device, plan.id));
	if (typeof result !== "string" && result.status === "ok" && result.data.status === "plan_ran") {
		flow.store.setState({ busy: false, ran: { plan, results: result.data.results } });
		return true;
	}
	flow.store.setState({ busy: false, note: noteOf(failureIn(result) ?? "The plan didn't run") });
	return false;
};

/** Drops the plan shown, so nothing in it runs. Returns whether it went. */
export const dismiss = async (
	flow: DispatchFlow,
	device: DeviceId | null,
	plan: Plan | null
): Promise<boolean> => {
	if (!device || !plan || flow.store.getState().busy) return false;
	flow.store.setState({ busy: true, note: null });
	const failure = failureIn(await tryCall(() => flow.source.dismissPlan(device, plan.id)));
	flow.store.setState({ busy: false, note: failure ? noteOf(failure) : null });
	return failure === null;
};

/** Leaves the conversation, so your next request starts a new one. */
export const startOver = (flow: DispatchFlow): void =>
	flow.store.setState({ fresh: true, ran: null, note: null });

/** The conversation's agent, unless you started over since. */
export const talkingTo = (
	view: ViewState,
	state: DispatchState,
	dispatcher: Dispatcher | undefined
): Agent | undefined =>
	state.fresh || !dispatcher?.agent ? undefined : view.tables.agent.get(dispatcher.agent);

/** The plan waiting on you, unless you started over since. */
export const shownPlan = (state: DispatchState, dispatcher: Dispatcher | undefined): Plan | null =>
	state.fresh ? null : (dispatcher?.plan ?? null);

/** Whether there's anything to show under the box: a conversation, a call, a note or results. */
export const hasPanel = (state: DispatchState, talking: Agent | undefined): boolean =>
	talking !== undefined || state.fresh || state.busy || state.note !== null || state.ran !== null;

/**
 * Whether the dispatcher is in use: a call on its way, the conversation
 * working or asking you something, or a plan waiting on you.
 */
export const inUse = (
	state: DispatchState,
	talking: Agent | undefined,
	plan: Plan | null
): boolean =>
	state.busy ||
	plan !== null ||
	(talking !== undefined && (isWorking(talking) || talking.activity.state === "needs_input"));
