// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A container workspace's snapshots. They aren't part of the view, so the
// window keeps the last list for each workspace and asks again when a view
// mounts, after a restore or when you press Refresh. It lasts as long as
// the window does.

import { useEffect } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { t } from "../i18n";

import type { DeviceId, Timestamp, WorkspaceId, WorkspaceSnapshot } from "../bindings";
import { failureOf } from "../agent-view/answer";
import type { DataSource } from "../data/source";
import { readOf } from "../pulls/read";
import { formatWhen } from "../status/format";
import type { ViewState } from "../view/replica";
import { linkForDevice } from "../view/select";

/** Whether a device's `maxd` lists and restores workspace snapshots. */
export const offersSnapshots = (view: ViewState, device: DeviceId): boolean =>
	linkForDevice(view, device)?.capabilities?.snapshots === true;

/** When a snapshot was taken and how long it's kept, in words. */
export const snapshotLine = (snapshot: WorkspaceSnapshot, now: Timestamp): string =>
	t("Taken {{value1}}, {{value2}}", {
		value1: formatWhen(snapshot.taken_at, now),
		value2:
			snapshot.expires_at === null
				? t("kept until you delete it")
				: t("kept until {{value1}}", { value1: formatWhen(snapshot.expires_at, now) }),
	});

/** Where one workspace's list stands. */
export type SnapshotList =
	| { readonly state: "loading"; readonly snapshots: readonly WorkspaceSnapshot[] | null }
	| { readonly state: "done"; readonly snapshots: readonly WorkspaceSnapshot[] }
	| {
			readonly state: "failed";
			readonly error: string;
			readonly snapshots: readonly WorkspaceSnapshot[] | null;
	  };

type SnapshotState = { readonly lists: ReadonlyMap<WorkspaceId, SnapshotList> };

const EMPTY: SnapshotState = { lists: new Map() };

const store = createStore<SnapshotState>(() => EMPTY);

const put = (workspace: WorkspaceId, list: SnapshotList) =>
	store.setState((state) => ({ lists: new Map(state.lists).set(workspace, list) }));

/**
 * Lists a workspace's snapshots again, keeping the last list on show while
 * it runs. A list already loading is left to finish.
 */
export const refreshSnapshots = async (
	source: Pick<DataSource, "listSnapshots">,
	workspace: WorkspaceId
): Promise<void> => {
	const last = store.getState().lists.get(workspace);
	if (last?.state === "loading") return;
	put(workspace, { state: "loading", snapshots: last?.snapshots ?? null });
	try {
		const { snapshots } = readOf(await source.listSnapshots(workspace), "snapshots");
		put(workspace, { state: "done", snapshots });
	} catch (error) {
		put(workspace, {
			state: "failed",
			error: error instanceof Error ? error.message : String(error),
			snapshots: last?.snapshots ?? null,
		});
	}
};

/**
 * Restores a snapshot, or makes a new workspace from it, then lists the
 * workspace's snapshots again. A roll back over unsaved work needs
 * `discardUnsaved`, once the person has seen what `rollbackLoses` says. Throws with
 * the device's words on a refusal.
 */
export const restoreSnapshot = async (
	source: Pick<DataSource, "listSnapshots" | "restoreSnapshot">,
	workspace: WorkspaceId,
	snapshot: string,
	asNew: boolean,
	discardUnsaved = false
): Promise<void> => {
	const failed = failureOf(
		await source.restoreSnapshot(workspace, snapshot, asNew, discardUnsaved)
	);
	if (failed) throw new Error(failed);
	await refreshSnapshots(source, workspace);
};

/** The last list of a workspace's snapshots, or none before the first. */
export const snapshotsOf = (workspace: WorkspaceId): SnapshotList | undefined =>
	store.getState().lists.get(workspace);

/**
 * A workspace's snapshots, asking for them when there's no list yet. Pass
 * null while there's no workspace.
 */
export const useSnapshots = (
	source: Pick<DataSource, "listSnapshots">,
	workspace: WorkspaceId | null
): SnapshotList | undefined => {
	const list = useStore(store, (state) => (workspace ? state.lists.get(workspace) : undefined));
	useEffect(() => {
		if (!workspace || store.getState().lists.has(workspace)) return;
		void refreshSnapshots(source, workspace);
	}, [source, workspace]);
	return list;
};

/** Forgets every list, for tests. */
export const clearSnapshots = (): void => store.setState(EMPTY);
