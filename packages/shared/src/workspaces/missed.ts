// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The files a finished project copy couldn't place, kept by the workspace it
// made, so that workspace's clone banner lists them on the Mac and the phone
// until you dismiss them. They stay while the app runs, and a restart
// forgets them, as nothing on the device keeps them.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import type { WorkspaceId } from "../bindings";
import type { CopiedProject, DataSource, PlaceFailure } from "../data/source";
import { formatNumber, t } from "../i18n";
import type { ViewState } from "../view/replica";
import { copyLanded } from "./copy";
import type { PlannedCopy } from "./copy-form";

type Missed = ReadonlyMap<WorkspaceId, readonly PlaceFailure[]>;

const stores = new WeakMap<DataSource, StoreApi<Missed>>();

const storeOf = (source: DataSource): StoreApi<Missed> => {
	let store = stores.get(source);
	if (!store) {
		store = createStore<Missed>(() => new Map());
		stores.set(source, store);
	}
	return store;
};

const NONE: readonly PlaceFailure[] = [];

/** Keeps the files a copy couldn't place in the workspace it made. */
export const noteMissed = (
	source: DataSource,
	workspace: WorkspaceId,
	failed: readonly PlaceFailure[]
): void => {
	if (failed.length === 0) return;
	const store = storeOf(source);
	store.setState(new Map(store.getState()).set(workspace, failed), true);
};

/** Forgets them. */
export const dismissMissed = (source: DataSource, workspace: WorkspaceId): void => {
	const store = storeOf(source);
	if (!store.getState().has(workspace)) return;
	const missed = new Map(store.getState());
	missed.delete(workspace);
	store.setState(missed, true);
};

/** The files a copy couldn't place in `workspace`, until they're dismissed. */
export const missedOf = (source: DataSource, workspace: WorkspaceId): readonly PlaceFailure[] =>
	storeOf(source).getState().get(workspace) ?? NONE;

/** The same, kept up to date. */
export const useMissed = (source: DataSource, workspace: WorkspaceId): readonly PlaceFailure[] =>
	useStore(storeOf(source), (missed) => missed.get(workspace) ?? NONE);

/**
 * Keeps what a finished copy couldn't place, once `runCopy` gave what it
 * placed, in the workspace the copy made. `before` holds the workspaces
 * there were when the copy started.
 */
export const keepMissed = (
	source: DataSource,
	view: ViewState,
	plan: PlannedCopy,
	before: ReadonlySet<WorkspaceId>,
	placed: CopiedProject | null
): void => {
	if (!placed || placed.failed.length === 0) return;
	const landed = copyLanded(view, plan, before);
	if (landed) noteMissed(source, landed.id, placed.failed);
};

/** What the banner's part for them is titled. */
export const missedTitle = (count: number): string =>
	t("{{countText}} files weren't copied", { count, countText: formatNumber(count) });
