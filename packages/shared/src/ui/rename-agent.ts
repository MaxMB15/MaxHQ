// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Renaming an agent, from the desktop's sidebar and header or the phone's
// agent screen: the name a draft comes to, the name shown at once while
// maxd stores it, and what went wrong when it doesn't. One of these lives
// with each window's services, and with the phone's.

import { createStore, type StoreApi } from "zustand/vanilla";

import type { Agent, AgentId } from "../bindings";
import type { DataSource } from "../data/source";
import { sentence } from "../status/format";
import { inboxFailureOf, triedLines } from "../view/inbox-actions";
import type { ViewStore } from "../view/store";

/** The longest name maxd keeps, in characters. It cuts the rest. */
export const NAME_CHARS = 80;

/** A draft as maxd keeps it: trimmed and cut to `NAME_CHARS`. A blank one clears the name. */
export const givenName = (draft: string): string => [...draft.trim()].slice(0, NAME_CHARS).join("");

/** A name on its way to maxd, and the title it stands over until maxd's answer shows. */
export type PendingName = { readonly name: string; readonly over: string };

/** The names on their way, by agent. */
export type RenameState = { readonly pending: ReadonlyMap<AgentId, PendingName> };

/** The names on their way and what renaming needs. */
export type Renames = {
	readonly store: StoreApi<RenameState>;
	readonly source: Pick<DataSource, "renameAgent">;
	readonly view: ViewStore;
};

/** No names on their way, for one window. */
export const createRenames = (
	source: Pick<DataSource, "renameAgent">,
	view: ViewStore
): Renames => ({
	store: createStore<RenameState>(() => ({ pending: new Map() })),
	source,
	view,
});

/** The title to show for an agent: the name you just gave it, until maxd's answer shows. */
export const shownTitle = (state: RenameState, agent: Agent): string => {
	const pending = state.pending.get(agent.id);
	return pending?.over === agent.title ? pending.name : agent.title;
};

const put = (renames: Renames, agent: AgentId, name: PendingName) => {
	const pending = new Map(renames.store.getState().pending);
	pending.set(agent, name);
	renames.store.setState({ pending });
};

// Takes the name away, unless a later rename put its own in its place.
const drop = (renames: Renames, agent: AgentId, name: PendingName) => {
	if (renames.store.getState().pending.get(agent) !== name) return;
	const pending = new Map(renames.store.getState().pending);
	pending.delete(agent);
	renames.store.setState({ pending });
};

// Takes the name away once the view's title moves off the old one. The
// answer and the change can come in either order, and the old title
// mustn't show between them.
const dropOnceShown = (renames: Renames, agent: AgentId, name: PendingName) => {
	const shown = () => renames.view.getState().view.tables.agent.get(agent)?.title !== name.over;
	if (shown()) return drop(renames, agent, name);
	const stop = renames.view.subscribe(() => {
		if (!shown()) return;
		stop();
		drop(renames, agent, name);
	});
};

/**
 * Renames an agent from a draft and shows the name at once. A blank draft
 * clears the name you gave, so the agent's own title shows again, and one
 * that leaves the title as it is sends nothing. Resolves, once the old
 * title is back, to what went wrong, or to null.
 */
export const renameAgent = async (
	renames: Renames,
	agent: Agent,
	draft: string
): Promise<string | null> => {
	const given = givenName(draft);
	if (given === agent.title) return null;
	// Only maxd knows the title a cleared name gives way to.
	const name = given === "" ? null : { name: given, over: agent.title };
	if (name) put(renames, agent.id, name);
	let reason: string | null;
	try {
		reason = inboxFailureOf(await renames.source.renameAgent(agent.id, given));
	} catch (error) {
		reason = sentence(error instanceof Error ? error.message : String(error));
	}
	if (reason === null) {
		if (name) dropOnceShown(renames, agent.id, name);
		return null;
	}
	if (name) drop(renames, agent.id, name);
	return triedLines("rename", [{ agent, reason }])[0]!;
};
