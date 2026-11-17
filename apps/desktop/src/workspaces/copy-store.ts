// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether the window's copy form is open and what it opened for, and the
// files a finished copy couldn't place, kept by the workspace it made so
// that workspace's clone banner can list them until you dismiss them.

import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { type Services, useServices } from "@/app/services";
import type { WorkspaceId } from "@maxhq/shared/bindings";
import type { PlaceFailure } from "@maxhq/shared/data/source";
import type { CopyRequest } from "@maxhq/shared/workspaces/copy-form";

export type { CopyRequest };

/** The form's state. */
export type CopyState = {
	readonly open: boolean;
	readonly request: CopyRequest;
	/** Counts each opening, so the form starts fresh every time. */
	readonly opened: number;
	readonly missed: ReadonlyMap<WorkspaceId, readonly PlaceFailure[]>;
};

const stores = new WeakMap<Services, StoreApi<CopyState>>();

/** The copy form's store for one window. */
export const copyOf = (services: Services): StoreApi<CopyState> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<CopyState>(() => ({
			open: false,
			request: {},
			opened: 0,
			missed: new Map(),
		}));
		stores.set(services, store);
	}
	return store;
};

/** Opens the form. */
export const openCopyProject = (services: Services, request: CopyRequest): void =>
	copyOf(services).setState((s) => ({ open: true, request, opened: s.opened + 1 }));

/** Closes the form. A copy it started keeps going. */
export const closeCopyProject = (services: Services): void =>
	copyOf(services).setState({ open: false });

/** Keeps the files a copy couldn't place in the workspace it made. */
export const noteMissed = (
	services: Services,
	workspace: WorkspaceId,
	failed: readonly PlaceFailure[]
): void => copyOf(services).setState((s) => ({ missed: new Map(s.missed).set(workspace, failed) }));

/** Forgets them. */
export const dismissMissed = (services: Services, workspace: WorkspaceId): void =>
	copyOf(services).setState((s) => {
		const missed = new Map(s.missed);
		missed.delete(workspace);
		return { missed };
	});

/** Reads the form's state. */
export const useCopy = <T>(selector: (state: CopyState) => T): T =>
	useStore(copyOf(useServices()), selector);
